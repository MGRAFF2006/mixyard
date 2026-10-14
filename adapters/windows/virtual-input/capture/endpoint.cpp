// SPDX-License-Identifier: MIT
// Original capture-only PortCls implementation; no sample implementation copied.
#include <initguid.h>
#include <ntddk.h>
#include <portcls.h>
#include <ksmedia.h>
#include <wdmsec.h>
#include "control.hpp"
#include "delivery.hpp"

void* __cdecl operator new(size_t bytes, POOL_TYPE, ULONG pool_tag) noexcept {
    return ExAllocatePool2(POOL_FLAG_NON_PAGED, bytes, pool_tag);
}
void __cdecl operator delete(void* p, POOL_TYPE, ULONG pool_tag) noexcept {
    if (p) ExFreePoolWithTag(p, pool_tag);
}

namespace {
using namespace mixyard_input;
constexpr ULONG tag = 'cIxM';
constexpr ULONG max_buffer_bytes = 4800 * 8; // 100 ms artifact budget, not quality evidence.
KSPIN_LOCK gate;
Transport transport;
CaptureStatus diagnostics;
PDEVICE_OBJECT control_device;
PDRIVER_DISPATCH port_dispatch[IRP_MJ_MAXIMUM_FUNCTION + 1];
PDRIVER_UNLOAD port_unload;
volatile LONG adapter_claimed;
volatile LONG stream_claimed;
uint64_t stream_epoch;
uint32_t* active_buffer;
ULONG active_bytes;

// A nonpaged object is owned by COM refs; streams retain their parent miniport.
#define OBJECT_REFS(Type) \
    LONG refs_ = 1; \
    STDMETHODIMP_(ULONG) AddRef() override { return static_cast<ULONG>(InterlockedIncrement(&refs_)); } \
    STDMETHODIMP_(ULONG) Release() override { const LONG n = InterlockedDecrement(&refs_); \
        if (!n) { this->~Type(); ExFreePoolWithTag(this, tag); } return static_cast<ULONG>(n); }

// Placement construction uses WDK's pool new, with matching explicit destruction.
KSDATAFORMAT_WAVEFORMATEXTENSIBLE native_format;
KSDATARANGE_AUDIO host_range;
KSDATARANGE bridge_range;
PKSDATARANGE host_ranges[] = {reinterpret_cast<PKSDATARANGE>(&host_range)};
PKSDATARANGE bridge_ranges[] = {&bridge_range};
PCPIN_DESCRIPTOR wave_pins[2];
PCPIN_DESCRIPTOR topo_pins[2];
PCCONNECTION_DESCRIPTOR connection[] = {{PCFILTER_NODE, 0, PCFILTER_NODE, 1}};
PCFILTER_DESCRIPTOR wave_filter;
PCFILTER_DESCRIPTOR topo_filter;
const GUID microphone_name = {0x622a2115,0xc0dc,0x4f04,{0xb6,0x22,0x8a,0xdf,0xd8,0x5d,0x8d,0x75}};

void Describe() {
    native_format.DataFormat.FormatSize = sizeof(native_format);
    native_format.DataFormat.SampleSize = 8;
    native_format.DataFormat.MajorFormat = KSDATAFORMAT_TYPE_AUDIO;
    native_format.DataFormat.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    native_format.DataFormat.Specifier = KSDATAFORMAT_SPECIFIER_WAVEFORMATEX;
    auto& w = native_format.WaveFormatExt;
    w.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE;
    w.Format.nChannels = 2;
    w.Format.nSamplesPerSec = 48000;
    w.Format.nAvgBytesPerSec = 384000;
    w.Format.nBlockAlign = 8;
    w.Format.wBitsPerSample = 32;
    w.Format.cbSize = sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX);
    w.Samples.wValidBitsPerSample = 32;
    w.dwChannelMask = KSAUDIO_SPEAKER_STEREO;
    w.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    host_range.DataRange = native_format.DataFormat;
    host_range.DataRange.FormatSize = sizeof(host_range);
    host_range.MaximumChannels = 2;
    host_range.MinimumBitsPerSample = host_range.MaximumBitsPerSample = 32;
    host_range.MinimumSampleFrequency = host_range.MaximumSampleFrequency = 48000;
    bridge_range.FormatSize = sizeof(bridge_range);
    bridge_range.MajorFormat = KSDATAFORMAT_TYPE_AUDIO;
    bridge_range.SubFormat = KSDATAFORMAT_SUBTYPE_ANALOG;
    bridge_range.Specifier = KSDATAFORMAT_SPECIFIER_NONE;
    // Topology: physical microphone IN -> bridge OUT; wave: bridge IN -> host OUT.
    for (ULONG i = 0; i != 2; ++i) {
        auto& t = topo_pins[i].KsPinDescriptor;
        t.DataRangesCount = 1; t.DataRanges = bridge_ranges;
        t.DataFlow = i ? KSPIN_DATAFLOW_OUT : KSPIN_DATAFLOW_IN;
        t.Communication = KSPIN_COMMUNICATION_NONE;
        t.Category = i ? &KSCATEGORY_AUDIO : &KSNODETYPE_MICROPHONE;
        t.Name = i ? nullptr : &microphone_name;
        auto& p = wave_pins[i].KsPinDescriptor;
        p.DataRangesCount = 1; p.DataRanges = i ? host_ranges : bridge_ranges;
        p.DataFlow = i ? KSPIN_DATAFLOW_OUT : KSPIN_DATAFLOW_IN;
        p.Communication = i ? KSPIN_COMMUNICATION_SINK : KSPIN_COMMUNICATION_NONE;
        p.Category = &KSCATEGORY_AUDIO;
    }
    wave_pins[1].MaxGlobalInstanceCount = wave_pins[1].MaxFilterInstanceCount = 1;
    wave_filter.PinSize = topo_filter.PinSize = sizeof(PCPIN_DESCRIPTOR);
    wave_filter.PinCount = topo_filter.PinCount = 2;
    wave_filter.Pins = wave_pins; topo_filter.Pins = topo_pins;
    wave_filter.ConnectionCount = topo_filter.ConnectionCount = 1;
    wave_filter.Connections = topo_filter.Connections = connection;
}

bool FormatOK(PKSDATAFORMAT f) {
    if (!f || f->FormatSize < sizeof(KSDATAFORMAT_WAVEFORMATEX) ||
        !IsEqualGUIDAligned(f->MajorFormat, KSDATAFORMAT_TYPE_AUDIO) ||
        !IsEqualGUIDAligned(f->SubFormat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) ||
        !IsEqualGUIDAligned(f->Specifier, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)) return false;
    const auto* w = &reinterpret_cast<PKSDATAFORMAT_WAVEFORMATEX>(f)->WaveFormatEx;
    if (w->nChannels != 2 || w->nSamplesPerSec != 48000 || w->nAvgBytesPerSec != 384000 ||
        w->nBlockAlign != 8 || w->wBitsPerSample != 32) return false;
    if (w->wFormatTag == WAVE_FORMAT_IEEE_FLOAT) return w->cbSize == 0;
    if (w->wFormatTag != WAVE_FORMAT_EXTENSIBLE ||
        w->cbSize != sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX) ||
        f->FormatSize < sizeof(native_format)) return false;
    const auto* ext = reinterpret_cast<const WAVEFORMATEXTENSIBLE*>(w);
    return ext->Samples.wValidBitsPerSample == 32 && ext->dwChannelMask == KSAUDIO_SPEAKER_STEREO &&
        IsEqualGUIDAligned(ext->SubFormat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
}

NTSTATUS Intersect(ULONG pin, PKSDATARANGE range, ULONG bytes, PVOID out, PULONG size) {
    *size = 0;
    if (pin != 1 || !range || range->FormatSize < sizeof(KSDATARANGE_AUDIO)) return STATUS_NO_MATCH;
    const auto* r = reinterpret_cast<PKSDATARANGE_AUDIO>(range);
    if ((!IsEqualGUIDAligned(r->DataRange.MajorFormat, KSDATAFORMAT_TYPE_AUDIO) &&
         !IsEqualGUIDAligned(r->DataRange.MajorFormat, KSDATAFORMAT_TYPE_WILDCARD)) ||
        (!IsEqualGUIDAligned(r->DataRange.SubFormat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) &&
         !IsEqualGUIDAligned(r->DataRange.SubFormat, KSDATAFORMAT_SUBTYPE_WILDCARD)) ||
        (!IsEqualGUIDAligned(r->DataRange.Specifier, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX) &&
         !IsEqualGUIDAligned(r->DataRange.Specifier, KSDATAFORMAT_SPECIFIER_WILDCARD)) ||
        r->MaximumChannels < 2 || r->MinimumBitsPerSample > 32 || r->MaximumBitsPerSample < 32 ||
        r->MinimumSampleFrequency > 48000 || r->MaximumSampleFrequency < 48000) return STATUS_NO_MATCH;
    *size = sizeof(native_format);
    if (!bytes) return STATUS_BUFFER_OVERFLOW;
    if (bytes < *size) return STATUS_BUFFER_TOO_SMALL;
    if (!out) return STATUS_INVALID_PARAMETER;
    RtlCopyMemory(out, &native_format, sizeof(native_format));
    return STATUS_SUCCESS;
}

class Stream final : public IMiniportWaveRTStreamNotification {
    PUNKNOWN parent_;
    PPORTWAVERTSTREAM port_;
    KTIMER timer_;
    KDPC dpc_;
    PMDL mdl_ = nullptr;
    uint32_t* buffer_ = nullptr;
    ULONG bytes_ = 0;
    ULONG notification_frames_ = 0;
    PKEVENT event_ = nullptr; // Referenced independently of the port's reference.
    KSSTATE state_ = KSSTATE_STOP;
    uint64_t frames_ = 0;
    uint64_t anchor_frames_ = 0;
    uint64_t anchor_qpc_ = 0;
    uint64_t frequency_ = 0;
    Delivery reader_;
public:
    Stream(PUNKNOWN parent, PPORTWAVERTSTREAM port) : parent_(parent), port_(port) {
        parent_->AddRef(); port_->AddRef();
        KeInitializeTimer(&timer_);
        KeInitializeDpc(&dpc_, Tick, this);
        LARGE_INTEGER f;
        (void)KeQueryPerformanceCounter(&f);
        frequency_ = static_cast<uint64_t>(f.QuadPart);
    }
    ~Stream() {
        Stop();
        FreeAudioBuffer(mdl_, bytes_);
        if (event_) ObDereferenceObject(event_);
        port_->Release(); parent_->Release();
        InterlockedExchange(&stream_claimed, 0);
    }
    OBJECT_REFS(Stream)
    STDMETHODIMP QueryInterface(REFIID id, PVOID* object) override {
        *object = nullptr;
        if (IsEqualGUIDAligned(id, IID_IUnknown) || IsEqualGUIDAligned(id, IID_IMiniportWaveRTStream) ||
            IsEqualGUIDAligned(id, IID_IMiniportWaveRTStreamNotification)) {
            *object = static_cast<IMiniportWaveRTStreamNotification*>(this); AddRef(); return STATUS_SUCCESS;
        }
        return STATUS_NOINTERFACE;
    }
    IMP_IMiniportWaveRTStream;
    IMP_IMiniportWaveRTStreamNotification;
private:
    void Stop() {
        KIRQL irql;
        KeAcquireSpinLock(&gate, &irql);
        state_ = KSSTATE_STOP;
        KeReleaseSpinLock(&gate, irql);
        KeCancelTimer(&timer_);
        KeRemoveQueueDpc(&dpc_);
        // PASSIVE teardown only: no queued/running callback may retain this pointer.
        KeFlushQueuedDpcs();
    }
    static void Tick(PKDPC, PVOID context, PVOID, PVOID) {
        auto* self = static_cast<Stream*>(context);
        KIRQL irql;
        KeAcquireSpinLock(&gate, &irql);
        self->Advance();
        KeReleaseSpinLock(&gate, irql);
    }
    void Advance() {
        if (state_ != KSSTATE_RUN || !buffer_) return;
        const auto now = static_cast<uint64_t>(KeQueryPerformanceCounter(nullptr).QuadPart);
        const uint64_t target = anchor_frames_ + clock_frames(now - anchor_qpc_, frequency_);
        const uint64_t before = frames_;
        const ULONG capacity = bytes_ / 8;
        if (target - frames_ > capacity) {
            diagnostics.missed_clock_frames += target - frames_ - capacity;
            frames_ = target - capacity;
            RtlZeroMemory(buffer_, bytes_);
            // A late callback cannot replay accumulated old producer packets.
            reader_.sequence = transport.next_sequence;
            reader_.offset = 0;
            reader_.generation = transport.generation;
        }
        while (frames_ < target) {
            const auto copied = reader_.copied_frames;
            reader_.frame(transport, buffer_ + (frames_ % capacity) * 2);
            if (reader_.copied_frames != copied) diagnostics.endpoint_frame = frames_;
            ++frames_;
        }
        KeMemoryBarrier(); // Publish positions only after writing all corresponding samples.
        diagnostics.endpoint_frames = frames_;
        diagnostics.copied_frames = reader_.copied_frames;
        diagnostics.silent_frames = reader_.silent_frames;
        diagnostics.lost_packets = reader_.lost_packets;
        diagnostics.revision = reader_.revision;
        diagnostics.producer_frame = reader_.producer_frame;
        diagnostics.qpc = now;
        diagnostics.qpc_frequency = frequency_;
        if (event_ && notification_frames_ && before / notification_frames_ != frames_ / notification_frames_)
            KeSetEvent(event_, IO_NO_INCREMENT, FALSE);
    }
};

_Use_decl_annotations_
NTSTATUS Stream::SetFormat(PKSDATAFORMAT format) { return FormatOK(format) ? STATUS_SUCCESS : STATUS_NO_MATCH; }
_Use_decl_annotations_
NTSTATUS Stream::SetState(KSSTATE state) {
    if (state < KSSTATE_STOP || state > KSSTATE_RUN) return STATUS_INVALID_PARAMETER;
    if (state != KSSTATE_RUN) {
        Stop();
        KIRQL irql; KeAcquireSpinLock(&gate, &irql);
        state_ = state;
        diagnostics.running = 0;
        if (state == KSSTATE_STOP) {
            frames_ = 0; reader_ = {};
            if (buffer_) RtlZeroMemory(buffer_, bytes_);
            RtlZeroMemory(&diagnostics, sizeof(diagnostics));
        }
        KeReleaseSpinLock(&gate, irql);
        return STATUS_SUCCESS;
    }
    KIRQL irql; KeAcquireSpinLock(&gate, &irql);
    if (!buffer_ || !frequency_ || frequency_ > max_u64 / 48000 ||
        stream_epoch == max_u64) { KeReleaseSpinLock(&gate, irql); return STATUS_DEVICE_NOT_READY; }
    if (state_ != KSSTATE_RUN) {
        diagnostics.stream_epoch = ++stream_epoch;
        anchor_frames_ = frames_;
        anchor_qpc_ = static_cast<uint64_t>(KeQueryPerformanceCounter(nullptr).QuadPart);
        state_ = state; diagnostics.running = 1;
        LARGE_INTEGER due; due.QuadPart = -10000; // 1 ms timer, coalescing is observable.
        KeSetTimerEx(&timer_, due, 1, &dpc_);
    }
    KeReleaseSpinLock(&gate, irql);
    return STATUS_SUCCESS;
}
_Use_decl_annotations_
NTSTATUS Stream::GetPosition(PKSAUDIO_POSITION position) {
    KIRQL irql; KeAcquireSpinLock(&gate, &irql);
    position->PlayOffset = position->WriteOffset = bytes_ ? (frames_ % (bytes_ / 8)) * 8 : 0;
    KeReleaseSpinLock(&gate, irql);
    return STATUS_SUCCESS;
}
_Use_decl_annotations_
NTSTATUS Stream::AllocateAudioBuffer(ULONG requested, PMDL* mdl, ULONG* actual, ULONG* offset, MEMORY_CACHING_TYPE* cache) {
    return AllocateBufferWithNotification(0, requested, mdl, actual, offset, cache);
}
_Use_decl_annotations_
NTSTATUS Stream::AllocateBufferWithNotification(ULONG count, ULONG requested, PMDL* mdl, ULONG* actual,
                                               ULONG* offset, MEMORY_CACHING_TYPE* cache) {
    *mdl = nullptr; *actual = *offset = 0; *cache = MmCached;
    if (mdl_ || state_ != KSSTATE_STOP || count > 2 || requested < 16 || requested > max_buffer_bytes)
        return STATUS_INVALID_PARAMETER;
    const ULONG alignment = count == 2 ? 16 : 8;
    requested -= requested % alignment;
    PHYSICAL_ADDRESS high; high.QuadPart = MAXLONGLONG;
    auto* allocated = port_->AllocatePagesForMdl(high, requested);
    if (!allocated) return STATUS_INSUFFICIENT_RESOURCES;
    auto* mapped = static_cast<uint32_t*>(port_->MapAllocatedPages(allocated, MmCached));
    if (!mapped) { port_->FreePagesFromMdl(allocated); return STATUS_INSUFFICIENT_RESOURCES; }
    RtlZeroMemory(mapped, requested);
    KIRQL irql; KeAcquireSpinLock(&gate, &irql);
    mdl_ = allocated; buffer_ = mapped; bytes_ = requested;
    active_buffer = mapped; active_bytes = requested;
    KeReleaseSpinLock(&gate, irql);
    notification_frames_ = count ? requested / 8 / count : 0;
    *mdl = allocated; *actual = requested;
    return STATUS_SUCCESS;
}
_Use_decl_annotations_
void Stream::FreeAudioBuffer(PMDL mdl, ULONG bytes) {
    UNREFERENCED_PARAMETER(bytes);
    if (!mdl || mdl != mdl_) return;
    Stop();
    KIRQL irql; KeAcquireSpinLock(&gate, &irql);
    auto* mapped = buffer_;
    RtlSecureZeroMemory(mapped, bytes_);
    active_buffer = nullptr; active_bytes = 0;
    mdl_ = nullptr; buffer_ = nullptr; bytes_ = notification_frames_ = 0;
    KeReleaseSpinLock(&gate, irql);
    port_->UnmapAllocatedPages(mapped, mdl);
    port_->FreePagesFromMdl(mdl);
}
_Use_decl_annotations_
void Stream::FreeBufferWithNotification(PMDL mdl, ULONG bytes) { FreeAudioBuffer(mdl, bytes); }
_Use_decl_annotations_
void Stream::GetHWLatency(KSRTAUDIO_HWLATENCY* latency) { RtlZeroMemory(latency, sizeof(*latency)); }
_Use_decl_annotations_
NTSTATUS Stream::GetPositionRegister(KSRTAUDIO_HWREGISTER* reg) { RtlZeroMemory(reg, sizeof(*reg)); return STATUS_NOT_SUPPORTED; }
_Use_decl_annotations_
NTSTATUS Stream::GetClockRegister(KSRTAUDIO_HWREGISTER* reg) { RtlZeroMemory(reg, sizeof(*reg)); return STATUS_NOT_SUPPORTED; }
_Use_decl_annotations_
NTSTATUS Stream::RegisterNotificationEvent(PKEVENT event) {
    if (!event) return STATUS_INVALID_PARAMETER;
    KIRQL irql; KeAcquireSpinLock(&gate, &irql);
    NTSTATUS result = STATUS_INSUFFICIENT_RESOURCES;
    if (!event_) { ObReferenceObject(event); event_ = event; result = STATUS_SUCCESS; }
    else if (event_ == event) result = STATUS_OBJECT_NAME_COLLISION;
    KeReleaseSpinLock(&gate, irql);
    return result;
}
_Use_decl_annotations_
NTSTATUS Stream::UnregisterNotificationEvent(PKEVENT event) {
    KIRQL irql; KeAcquireSpinLock(&gate, &irql);
    const bool match = event && event_ == event;
    if (match) event_ = nullptr;
    KeReleaseSpinLock(&gate, irql);
    if (match) ObDereferenceObject(event);
    return match ? STATUS_SUCCESS : STATUS_NOT_FOUND;
}

class Wave final : public IMiniportWaveRT {
public:
    OBJECT_REFS(Wave)
    STDMETHODIMP QueryInterface(REFIID id, PVOID* object) override {
        *object = nullptr;
        if (IsEqualGUIDAligned(id, IID_IUnknown) || IsEqualGUIDAligned(id, IID_IMiniport) || IsEqualGUIDAligned(id, IID_IMiniportWaveRT)) {
            *object = static_cast<IMiniportWaveRT*>(this); AddRef(); return STATUS_SUCCESS;
        }
        return STATUS_NOINTERFACE;
    }
    STDMETHODIMP GetDescription(PPCFILTER_DESCRIPTOR* desc) override { *desc = &wave_filter; return STATUS_SUCCESS; }
    STDMETHODIMP DataRangeIntersection(ULONG pin, PKSDATARANGE range, PKSDATARANGE match, ULONG bytes, PVOID out, PULONG size) override {
        UNREFERENCED_PARAMETER(match); return Intersect(pin, range, bytes, out, size);
    }
    STDMETHODIMP Init(PUNKNOWN adapter, PRESOURCELIST resources, PPORTWAVERT port) override {
        UNREFERENCED_PARAMETER(adapter); UNREFERENCED_PARAMETER(resources); UNREFERENCED_PARAMETER(port); return STATUS_SUCCESS;
    }
    STDMETHODIMP GetDeviceDescription(PDEVICE_DESCRIPTION desc) override {
        RtlZeroMemory(desc, sizeof(*desc)); desc->Version = DEVICE_DESCRIPTION_VERSION;
        desc->Master = TRUE; desc->ScatterGather = TRUE; desc->Dma32BitAddresses = TRUE;
        desc->InterfaceType = Internal; desc->MaximumLength = max_buffer_bytes; return STATUS_SUCCESS;
    }
    STDMETHODIMP NewStream(PMINIPORTWAVERTSTREAM* stream, PPORTWAVERTSTREAM port, ULONG pin, BOOLEAN capture, PKSDATAFORMAT format) override {
        *stream = nullptr;
        if (pin != 1 || !capture || !FormatOK(format)) return STATUS_NO_MATCH;
        if (InterlockedCompareExchange(&stream_claimed, 1, 0)) return STATUS_DEVICE_BUSY;
        auto* s = new (NonPagedPoolNx, tag) Stream(this, port);
        if (!s) { InterlockedExchange(&stream_claimed, 0); return STATUS_INSUFFICIENT_RESOURCES; }
        *stream = s; return STATUS_SUCCESS;
    }
};
class Topology final : public IMiniportTopology {
public:
    OBJECT_REFS(Topology)
    STDMETHODIMP QueryInterface(REFIID id, PVOID* object) override {
        *object = nullptr;
        if (IsEqualGUIDAligned(id, IID_IUnknown) || IsEqualGUIDAligned(id, IID_IMiniport) || IsEqualGUIDAligned(id, IID_IMiniportTopology)) {
            *object = static_cast<IMiniportTopology*>(this); AddRef(); return STATUS_SUCCESS;
        }
        return STATUS_NOINTERFACE;
    }
    STDMETHODIMP GetDescription(PPCFILTER_DESCRIPTOR* desc) override { *desc = &topo_filter; return STATUS_SUCCESS; }
    STDMETHODIMP DataRangeIntersection(ULONG pin, PKSDATARANGE range, PKSDATARANGE match, ULONG bytes, PVOID out, PULONG size) override {
        UNREFERENCED_PARAMETER(pin); UNREFERENCED_PARAMETER(range); UNREFERENCED_PARAMETER(match);
        UNREFERENCED_PARAMETER(bytes); UNREFERENCED_PARAMETER(out); *size = 0; return STATUS_NOT_IMPLEMENTED;
    }
    STDMETHODIMP Init(PUNKNOWN adapter, PRESOURCELIST resources, PPORTTOPOLOGY port) override {
        UNREFERENCED_PARAMETER(adapter); UNREFERENCED_PARAMETER(resources); UNREFERENCED_PARAMETER(port); return STATUS_SUCCESS;
    }
};

NTSTATUS Complete(PIRP irp, NTSTATUS status, ULONG_PTR bytes = 0) {
    irp->IoStatus.Status = status; irp->IoStatus.Information = bytes;
    IoCompleteRequest(irp, IO_NO_INCREMENT); return status;
}
NTSTATUS ResultStatus(Result r) {
    switch (r) {
    case Result::accepted: case Result::accepted_overwrite: return STATUS_SUCCESS;
    case Result::unauthorized: return STATUS_ACCESS_DENIED;
    case Result::stale: return STATUS_REVISION_MISMATCH;
    case Result::exhausted: return STATUS_INTEGER_OVERFLOW;
    default: return STATUS_INVALID_PARAMETER;
    }
}
DRIVER_DISPATCH Dispatch;
_Use_decl_annotations_
NTSTATUS Dispatch(PDEVICE_OBJECT device, PIRP irp) {
    auto* stack = IoGetCurrentIrpStackLocation(irp);
    if (device != control_device) return port_dispatch[stack->MajorFunction](device, irp);
    auto* file = stack->FileObject;
    NTSTATUS status = STATUS_INVALID_DEVICE_REQUEST;
    ULONG_PTR bytes = 0;
    KIRQL irql; KeAcquireSpinLock(&gate, &irql);
    const auto key = reinterpret_cast<ULONG_PTR>(file);
    if (stack->MajorFunction == IRP_MJ_CREATE) {
        if (irp->RequestorMode != UserMode || !file) status = STATUS_ACCESS_DENIED;
        else if (file->FileName.Length || file->RelatedFileObject) status = STATUS_OBJECT_NAME_INVALID;
        else if (!stack->Parameters.Create.SecurityContext ||
            (stack->Parameters.Create.SecurityContext->DesiredAccess & (FILE_READ_DATA | FILE_WRITE_DATA)) !=
             (FILE_READ_DATA | FILE_WRITE_DATA)) status = STATUS_ACCESS_DENIED;
        else status = ResultStatus(transport.begin(key, transport.generation == max_u64 ? max_u64 : transport.generation + 1));
    } else if (stack->MajorFunction == IRP_MJ_CLEANUP || stack->MajorFunction == IRP_MJ_CLOSE) {
        if (key && transport.owner == key) {
            (void)transport.cleanup(key);
            if (active_buffer) RtlSecureZeroMemory(active_buffer, active_bytes);
        }
        status = STATUS_SUCCESS;
    } else if (stack->MajorFunction == IRP_MJ_DEVICE_CONTROL) {
        const auto code = stack->Parameters.DeviceIoControl.IoControlCode;
        const auto input = stack->Parameters.DeviceIoControl.InputBufferLength;
        const auto output = stack->Parameters.DeviceIoControl.OutputBufferLength;
        auto* buffer = irp->AssociatedIrp.SystemBuffer;
        if (irp->RequestorMode != UserMode || !key || transport.owner != key) status = STATUS_ACCESS_DENIED;
        else if (!buffer) status = STATUS_INVALID_PARAMETER;
        else if (code == ioctl_capture_status && !input && output >= sizeof(CaptureStatus)) {
            auto snapshot = diagnostics;
            snapshot.version = 1; snapshot.bytes = sizeof(snapshot);
            snapshot.generation = transport.generation; snapshot.producer_epoch = transport.epoch;
            snapshot.next_sequence = transport.next_sequence; snapshot.next_frame = transport.next_frame;
            snapshot.overwritten_packets = transport.overwritten_packets;
            RtlCopyMemory(buffer, &snapshot, sizeof(snapshot)); bytes = sizeof(snapshot); status = STATUS_SUCCESS;
        } else if (code == ioctl_submit && output >= sizeof(SubmitReply)) {
            const auto result = transport.submit(key, buffer, input);
            status = ResultStatus(result);
            if (NT_SUCCESS(status)) {
                const SubmitReply reply{control_version, sizeof(SubmitReply), static_cast<uint32_t>(result), 0};
                RtlCopyMemory(buffer, &reply, sizeof(reply)); bytes = sizeof(reply);
            } else {
                // Invalid producer data revokes this admission; no queued replay.
                (void)transport.cleanup(key);
                if (active_buffer) RtlSecureZeroMemory(active_buffer, active_bytes);
            }
        }
    }
    KeReleaseSpinLock(&gate, irql);
    return Complete(irp, status, bytes);
}

NTSTATUS Register(PDEVICE_OBJECT device, PIRP irp, PRESOURCELIST resources, REFCLSID cls,
                  PCWSTR name, PUNKNOWN miniport, PPORT* result) {
    *result = nullptr;
    PPORT port = nullptr;
    NTSTATUS status = PcNewPort(&port, cls);
    if (NT_SUCCESS(status)) status = port->Init(device, irp, miniport, nullptr, resources);
    if (NT_SUCCESS(status)) status = PcRegisterSubdevice(device, name, port);
    if (NT_SUCCESS(status)) *result = port;
    else if (port) port->Release();
    return status;
}
NTSTATUS Start(PDEVICE_OBJECT device, PIRP irp, PRESOURCELIST resources) {
    auto* wave = new (NonPagedPoolNx, tag) Wave;
    auto* topo = new (NonPagedPoolNx, tag) Topology;
    if (!wave || !topo) { if (wave) wave->Release(); if (topo) topo->Release(); return STATUS_INSUFFICIENT_RESOURCES; }
    PPORT wp = nullptr, tp = nullptr;
    NTSTATUS status = Register(device, irp, resources, CLSID_PortTopology, L"TopologyCapture", topo, &tp);
    if (NT_SUCCESS(status)) status = Register(device, irp, resources, CLSID_PortWaveRT, L"WaveCapture", wave, &wp);
    if (NT_SUCCESS(status)) status = PcRegisterPhysicalConnection(device, tp, 1, wp, 0);
    if (wp) wp->Release(); if (tp) tp->Release(); wave->Release(); topo->Release();
    return status; // PortCls owns registered subdevices and failed-start PnP cleanup.
}
DRIVER_ADD_DEVICE AddDevice;
_Use_decl_annotations_
NTSTATUS AddDevice(PDRIVER_OBJECT driver, PDEVICE_OBJECT pdo) {
    // ponytail: one adapter until unload; dynamic identities/provisioning remain blocked.
    if (InterlockedCompareExchange(&adapter_claimed, 1, 0)) return STATUS_DEVICE_BUSY;
    const auto status = PcAddAdapterDevice(driver, pdo, Start, 2, 0);
    if (!NT_SUCCESS(status)) InterlockedExchange(&adapter_claimed, 0);
    return status;
}
DRIVER_UNLOAD Unload;
_Use_decl_annotations_
void Unload(PDRIVER_OBJECT driver) {
    UNICODE_STRING link; RtlInitUnicodeString(&link, L"\\DosDevices\\Global\\MixyardCaptureIngress");
    (void)IoDeleteSymbolicLink(&link);
    IoDeleteDevice(control_device); control_device = nullptr;
    RtlSecureZeroMemory(&transport, sizeof(transport));
    if (port_unload) port_unload(driver);
}
} // namespace

extern "C" DRIVER_INITIALIZE DriverEntry;
_Use_decl_annotations_
NTSTATUS DriverEntry(PDRIVER_OBJECT driver, PUNICODE_STRING path) {
    Describe(); KeInitializeSpinLock(&gate); transport.initialize();
    NTSTATUS status = PcInitializeAdapterDriver(driver, path, AddDevice);
    if (!NT_SUCCESS(status)) return status;
    for (ULONG i = 0; i <= IRP_MJ_MAXIMUM_FUNCTION; ++i) port_dispatch[i] = driver->MajorFunction[i];
    port_unload = driver->DriverUnload;
    UNICODE_STRING name, link, sddl;
    RtlInitUnicodeString(&name, L"\\Device\\MixyardCaptureIngress");
    RtlInitUnicodeString(&link, L"\\DosDevices\\Global\\MixyardCaptureIngress");
    RtlInitUnicodeString(&sddl, L"D:P(A;;GA;;;SY)(A;;GA;;;BA)");
    const GUID security = {0xd9eeb670,0xb730,0x4313,{0x94,0x2c,0x13,0x8a,0x1b,0x75,0xad,0xbc}};
    status = IoCreateDeviceSecure(driver, 0, &name, device_type, FILE_DEVICE_SECURE_OPEN, FALSE,
                                  &sddl, &security, &control_device);
    if (!NT_SUCCESS(status)) return status;
    status = IoCreateSymbolicLink(&link, &name);
    if (!NT_SUCCESS(status)) { IoDeleteDevice(control_device); control_device = nullptr; return status; }
    for (ULONG i = 0; i <= IRP_MJ_MAXIMUM_FUNCTION; ++i) driver->MajorFunction[i] = Dispatch;
    driver->DriverUnload = Unload;
    control_device->Flags &= ~DO_DEVICE_INITIALIZING;
    return STATUS_SUCCESS;
}
