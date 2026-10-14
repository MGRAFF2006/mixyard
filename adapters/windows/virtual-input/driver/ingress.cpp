// SPDX-License-Identifier: MIT
// Original private WDM ingress scaffold. NOT a PortCls/WaveRT audio driver.
// No KS interfaces, consumer, drain IOCTL, timer, or sample-effective ACK exists.
#include <ntddk.h>
#include <wdmsec.h>
#include "ioctl.hpp"
#include "../transport.hpp"

namespace {
// Custom security class, separate from the INF's standard System setup class.
const GUID security_class = {
    0x91fc8b4d, 0x0dd4, 0x444c, {0x8a, 0x82, 0xb5, 0x67, 0x23, 0xdc, 0xf2, 0xa5}
};
constexpr wchar_t device_name[] = L"\\Device\\MixyardIngress";
constexpr wchar_t link_name[] = L"\\DosDevices\\Global\\MixyardIngress";

struct Extension {
    KSPIN_LOCK lock;
    mixyard_input::Transport transport;
};

NTSTATUS Complete(PIRP irp, NTSTATUS status, ULONG_PTR bytes = 0) {
    irp->IoStatus.Status = status;
    irp->IoStatus.Information = bytes;
    IoCompleteRequest(irp, IO_NO_INCREMENT);
    return status;
}

NTSTATUS MapResult(mixyard_input::Result result) {
    using mixyard_input::Result;
    switch (result) {
    case Result::accepted:
    case Result::accepted_overwrite: return STATUS_SUCCESS;
    case Result::unauthorized: return STATUS_ACCESS_DENIED;
    case Result::stale: return STATUS_REVISION_MISMATCH;
    case Result::out_of_order:
    case Result::malformed: return STATUS_INVALID_PARAMETER;
    case Result::exhausted: return STATUS_INTEGER_OVERFLOW;
    case Result::unavailable: return STATUS_DEVICE_NOT_READY;
    }
    return STATUS_INVALID_DEVICE_REQUEST;
}

DRIVER_DISPATCH Unsupported;
DRIVER_DISPATCH Create;
DRIVER_DISPATCH Cleanup;
DRIVER_DISPATCH Close;
DRIVER_DISPATCH DeviceControl;
DRIVER_UNLOAD Unload;

_Use_decl_annotations_
NTSTATUS Unsupported(PDEVICE_OBJECT device, PIRP irp) {
    UNREFERENCED_PARAMETER(device);
    return Complete(irp, STATUS_INVALID_DEVICE_REQUEST);
}

_Use_decl_annotations_
NTSTATUS Create(PDEVICE_OBJECT device, PIRP irp) {
    const auto* stack = IoGetCurrentIrpStackLocation(irp);
    auto* file = stack->FileObject;
    // Kernel callers can bypass normal device ACL checks; they are not producers.
    if (irp->RequestorMode != UserMode || !file)
        return Complete(irp, STATUS_ACCESS_DENIED);
    // No subordinate namespace and no open relative to another file object.
    if (file->FileName.Length || file->RelatedFileObject)
        return Complete(irp, STATUS_OBJECT_NAME_INVALID);
    if (!stack->Parameters.Create.SecurityContext ||
        (stack->Parameters.Create.SecurityContext->DesiredAccess &
         (FILE_READ_DATA | FILE_WRITE_DATA)) != (FILE_READ_DATA | FILE_WRITE_DATA))
        return Complete(irp, STATUS_ACCESS_DENIED);

    auto* state = static_cast<Extension*>(device->DeviceExtension);
    KIRQL old_irql;
    KeAcquireSpinLock(&state->lock, &old_irql);
    NTSTATUS status;
    if (state->transport.owner) {
        status = STATUS_SHARING_VIOLATION;
    } else if (state->transport.generation == UINT64_MAX) {
        status = STATUS_INTEGER_OVERFLOW;
    } else {
        // Epoch is an admission identifier, not a running audio clock.
        status = MapResult(state->transport.begin(reinterpret_cast<uintptr_t>(file),
                                                  state->transport.generation + 1));
    }
    KeReleaseSpinLock(&state->lock, old_irql);
    return Complete(irp, status);
}

_Use_decl_annotations_
NTSTATUS Cleanup(PDEVICE_OBJECT device, PIRP irp) {
    auto* state = static_cast<Extension*>(device->DeviceExtension);
    const auto* stack = IoGetCurrentIrpStackLocation(irp);
    KIRQL old_irql;
    KeAcquireSpinLock(&state->lock, &old_irql);
    // Failed opens and already-cleaned file objects cannot retire another owner.
    if (state->transport.owner == reinterpret_cast<uintptr_t>(stack->FileObject) &&
        state->transport.owner != 0) {
        (void)state->transport.cleanup(state->transport.owner);
        RtlSecureZeroMemory(state->transport.packets, sizeof(state->transport.packets));
    }
    KeReleaseSpinLock(&state->lock, old_irql);
    return Complete(irp, STATUS_SUCCESS);
}

_Use_decl_annotations_
NTSTATUS Close(PDEVICE_OBJECT device, PIRP irp) {
    // Cleanup is sent after the final duplicated handle closes. Close can arrive
    // later, after a fresh producer has opened a DIFFERENT file object.
    return Cleanup(device, irp);
}

_Use_decl_annotations_
NTSTATUS DeviceControl(PDEVICE_OBJECT device, PIRP irp) {
    const auto* stack = IoGetCurrentIrpStackLocation(irp);
    if (irp->RequestorMode != UserMode || !stack->FileObject)
        return Complete(irp, STATUS_ACCESS_DENIED);

    const ULONG code = stack->Parameters.DeviceIoControl.IoControlCode;
    const ULONG input_bytes = stack->Parameters.DeviceIoControl.InputBufferLength;
    const ULONG output_bytes = stack->Parameters.DeviceIoControl.OutputBufferLength;
    void* buffer = irp->AssociatedIrp.SystemBuffer;
    using namespace mixyard_input;
    if (code != ioctl_submit && code != ioctl_status)
        return Complete(irp, STATUS_INVALID_DEVICE_REQUEST);
    if (!buffer)
        return Complete(irp, STATUS_INVALID_PARAMETER);
    // Validate output before changing state: failed replies never queue packets.
    if ((code == ioctl_submit && output_bytes < sizeof(SubmitReply)) ||
        (code == ioctl_status && output_bytes < sizeof(IngressStatus)))
        return Complete(irp, STATUS_BUFFER_TOO_SMALL);
    if ((code == ioctl_status && input_bytes != 0) ||
        (code == ioctl_submit &&
         (input_bytes < header_bytes || input_bytes > max_packet_bytes)))
        return Complete(irp, STATUS_INVALID_PARAMETER);

    auto* state = static_cast<Extension*>(device->DeviceExtension);
    KIRQL old_irql;
    KeAcquireSpinLock(&state->lock, &old_irql);
    NTSTATUS status = STATUS_ACCESS_DENIED;
    ULONG_PTR bytes = 0;
    if (state->transport.owner == reinterpret_cast<uintptr_t>(stack->FileObject)) {
        if (code == ioctl_submit) {
            const Result result = state->transport.submit(state->transport.owner,
                                                         buffer, input_bytes);
            status = MapResult(result);
            if (NT_SUCCESS(status)) {
                auto* reply = static_cast<SubmitReply*>(buffer);
                reply->version = control_version;
                reply->bytes = sizeof(SubmitReply);
                reply->result = static_cast<uint32_t>(result);
                reply->reserved = 0;
                bytes = sizeof(SubmitReply);
            }
        } else {
            auto* reply = static_cast<IngressStatus*>(buffer);
            RtlZeroMemory(reply, sizeof(*reply));
            reply->version = control_version;
            reply->bytes = sizeof(IngressStatus);
            reply->generation = state->transport.generation;
            reply->epoch = state->transport.epoch;
            reply->next_frame = state->transport.next_frame;
            reply->next_sequence = state->transport.next_sequence;
            reply->revision = state->transport.revision;
            reply->overwritten_packets = state->transport.overwritten_packets;
            bytes = sizeof(IngressStatus);
            status = STATUS_SUCCESS;
        }
    }
    KeReleaseSpinLock(&state->lock, old_irql);
    return Complete(irp, status, bytes);
}

_Use_decl_annotations_
void Unload(PDRIVER_OBJECT driver) {
    UNICODE_STRING link;
    RtlInitUnicodeString(&link, link_name);
    (void)IoDeleteSymbolicLink(&link);
    if (driver->DeviceObject) {
        auto* state = static_cast<Extension*>(driver->DeviceObject->DeviceExtension);
        RtlSecureZeroMemory(&state->transport, sizeof(state->transport));
        IoDeleteDevice(driver->DeviceObject);
    }
}
} // namespace

extern "C" DRIVER_INITIALIZE DriverEntry;

_Use_decl_annotations_
NTSTATUS DriverEntry(PDRIVER_OBJECT driver, PUNICODE_STRING registry_path) {
    UNREFERENCED_PARAMETER(registry_path);
    for (ULONG i = 0; i <= IRP_MJ_MAXIMUM_FUNCTION; ++i)
        driver->MajorFunction[i] = Unsupported;
    driver->MajorFunction[IRP_MJ_CREATE] = Create;
    driver->MajorFunction[IRP_MJ_CLEANUP] = Cleanup;
    driver->MajorFunction[IRP_MJ_CLOSE] = Close;
    driver->MajorFunction[IRP_MJ_DEVICE_CONTROL] = DeviceControl;
    driver->DriverUnload = Unload;

    UNICODE_STRING name;
    UNICODE_STRING link;
    UNICODE_STRING sddl;
    RtlInitUnicodeString(&name, device_name);
    RtlInitUnicodeString(&link, link_name);
    RtlInitUnicodeString(&sddl, L"D:P(A;;GA;;;SY)(A;;GA;;;BA)");
    PDEVICE_OBJECT device = nullptr;
    NTSTATUS status = IoCreateDeviceSecure(driver, sizeof(Extension), &name,
        mixyard_input::device_type, FILE_DEVICE_SECURE_OPEN, FALSE,
        &sddl, &security_class, &device);
    if (!NT_SUCCESS(status)) return status;

    auto* state = static_cast<Extension*>(device->DeviceExtension);
    KeInitializeSpinLock(&state->lock);
    state->transport.initialize();
    status = IoCreateSymbolicLink(&link, &name);
    if (!NT_SUCCESS(status)) {
        RtlSecureZeroMemory(&state->transport, sizeof(state->transport));
        IoDeleteDevice(device);
        return status;
    }
    device->Flags &= ~DO_DEVICE_INITIALIZING;
    return STATUS_SUCCESS;
}
