"""Optional actual C ABI qualification; never build or discover a native engine."""

from contextlib import ExitStack
from dataclasses import replace
import json
import os
import socket
import struct
import subprocess
import threading
import unittest
from unittest.mock import patch

from mixyard_interop import Endpoint, Grant, Provenance, Session, Source, Spec
from mixyard_interop.native import HostLoan, NativeHost, NativeRejected, NativeUnavailable, convert_processed
from mixyard_interop.http_upnp import StreamSource, StreamReceiver
from mixyard_interop.http_upnp import stream as stream_io
from mixyard_interop.realtime import RtpProfile, RtpReceiver, RtpSender
from mixyard_interop.realtime.leased import LeasedRtpReceiver, LeasedRtpSender
from mixyard_interop.runtime import (ClockEvidence, LeaseBinding, LeasedBlock, RuntimeBusy,
                                    RuntimeLease)

ENV = ("MIXYARD_NATIVE_HOST", "MIXYARD_NATIVE_SHA256", "MIXYARD_NATIVE_REVISION", "MIXYARD_NATIVE_TOOLCHAIN")
HAS_LOAN = all(os.environ.get(key) for key in ENV)


def loan():
    return HostLoan(*(os.environ[key] for key in ENV), owner_check=lambda: True)


def request():
    # Read the exact owner's tracked synthetic fixture into memory, never a
    # mutable author checkout or another context's output/cache.
    value = subprocess.run(["git", "show", os.environ["MIXYARD_NATIVE_REVISION"]
                            + ":adapters/engine/fixtures/owned-common-clock.json"],
                           check=True, capture_output=True).stdout
    if len(value) > 1024 * 1024:
        raise ValueError("fixture exceeds request budget")
    return json.loads(value)


def lease(spec, observed, epoch):
    clock = ClockEvidence(11, epoch, 0, 1_000_000)
    binding = LeaseBinding(observed, spec, 7, 1, epoch, 11, clock)
    state = [binding]
    result = RuntimeLease(spec, observed_id=observed, generation=7, revision=1,
                          source_epoch=epoch, clock=clock, owner_check=lambda: state[0], consent=True)
    return result, state


def setup_host(codec="raw-f64-le"):
    value = request()
    encoding = {"raw-f64-le": "f64-le", "raw-f32-le": "f32-le", "raw-s16-le": "s16-le",
                "raw-s24-le": "s24-le", "raw-s32-le": "s32-le"}[codec]
    sources, states = [None], [None]
    for index in (1, 2):
        info = value["sources"][index]
        info["encoding"] = encoding
        group = ("call-return",) if index == 1 else ()
        spec = Spec(index + 1, info["observedId"], codec, info["sampleRateHz"], tuple(info["channels"]),
                    2, Provenance((Source("fixture", info["observedId"], group),), ("fixture",)))
        owned, state = lease(spec, info["observedId"], f"source-{index}-epoch")
        sources.append(owned)
        states.append(state)
    outputs, output_states = [], []
    # Independent fixture oracle: Call <- shared only. Record <- return+shared.
    # Never derive the expected set by reading config sends or native metadata.
    for index, contributors in enumerate(((2,), (1, 2))):
        info = value["outputs"][index]
        provenance = Provenance(tuple(sources[i].spec.provenance.sources[0] for i in contributors),
                                ("fixture", "runtime"))
        spec = Spec(100 + index, info["observedId"], "raw-f64-le", info["sampleRateHz"],
                    tuple(info["channels"]), 2, provenance)
        owned, state = lease(spec, info["observedId"], f"output-{index}-epoch")
        outputs.append(owned)
        output_states.append(state)
    return value, sources, outputs, states, output_states


def pcm(codec, values):
    if codec in ("raw-f32-le", "raw-f64-le"):
        kind = "f" if codec == "raw-f32-le" else "d"
        return struct.pack("<4" + kind, *values)
    width = {"raw-s16-le": 2, "raw-s24-le": 3, "raw-s32-le": 4}[codec]
    return b"".join(v.to_bytes(width, "little", signed=True) for v in values)


def blocks(sources, frame=0, values=None):
    values = values or ((.5, -.25, .125, -.0625), (.125, -.5, .25, -.125))
    result = {}
    for index, samples in zip((1, 2), values):
        spec = sources[index].spec
        result[index] = LeasedBlock(spec, pcm(spec.codec, samples), 7, 1, f"source-{index}-epoch", frame, 11)
    return result


def unused(scheme):
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM if scheme == "udp" else socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return Endpoint("127.0.0.1", sock.getsockname()[1], scheme)


def udp_pair(stack, spec, sender_peer, receiver_peer):
    destination = unused("udp")
    outgoing = Session(sender_peer, [Grant("live-send", destination, receiver_peer, spec)], consent=True)
    sender = stack.enter_context(RtpSender(outgoing, destination, RtpProfile(spec), ssrc=17,
                                          timestamp=100, sequence=1))
    incoming = Session(receiver_peer, [Grant("live-receive", sender.local, sender_peer, spec)], consent=True)
    receiver = stack.enter_context(RtpReceiver(incoming, destination, sender.local, RtpProfile(spec), ssrc=17))
    return sender, receiver


def agreement(epoch):
    return dict(generation=7, revision=1, source_epoch=epoch, base_frame=0, clock_id=11)


class DestinationConversionTests(unittest.TestCase):
    def test_conversion_retains_producer_stop_through_independent_destination_owner_callback(self):
        from mixyard_interop.runtime import _ProducerReceipt
        stopped = threading.Event()
        spec = Spec(1, "processed", "raw-f64-le", 48000, ("FL", "FR"), 2,
                    Provenance((Source("runtime", "owned"),), ("runtime",)))
        source = LeasedBlock(spec, struct.pack("<4d", -.5, .25, .125, -.25),
                             7, 1, "output", 0, 11, _producer=_ProducerReceipt(stopped))
        target = replace(spec, codec="raw-s24-le")
        converted = convert_processed(source, target, consent=True, rounding="nearest-ties-even").output
        destination, state = lease(target, "owned-converted-destination", "output")
        def stop_during_owner_check():
            stopped.set()
            return state[0]
        destination._owner_check = stop_during_owner_check
        consumed = []
        with self.assertRaises(PermissionError):
            destination.accept(converted, consumed.append)
        self.assertEqual(consumed, [])
        with self.assertRaises(PermissionError):
            convert_processed(source, target, consent=True, rounding="nearest-ties-even")

    def test_conversion_is_explicit_local_finite_and_never_raw_identity(self):
        spec = Spec(1, "processed", "raw-f64-le", 48000, ("FL", "FR"), 2,
                    Provenance((Source("runtime", "owned"),), ("runtime",)))
        block = LeasedBlock(spec, struct.pack("<4d", -.5, .25, 1 / 8388608, -1), 7, 1, "output", 0, 11)
        target = replace(spec, codec="raw-s24-le")
        with self.assertRaises(PermissionError):
            convert_processed(block, target, rounding="nearest-ties-even")
        converted = convert_processed(block, target, consent=True, rounding="nearest-ties-even")
        self.assertEqual(converted.output.pcm, b"".join(x.to_bytes(3, "little", signed=True)
                                                       for x in (-4194304, 2097152, 1, -8388608)))
        self.assertEqual((converted.source_spec, block.spec.codec), (spec, "raw-f64-le"))
        for value in (1., float("nan"), .999999999999):
            with self.assertRaises(ValueError):
                convert_processed(replace(block, pcm=struct.pack("<4d", value, 0, 0, 0)), target,
                                  consent=True, rounding="nearest-ties-even")


@unittest.skipUnless(HAS_LOAN, "explicit pinned read-only native owner loan not supplied; no build/fallback")
class ActualNativeHostTests(unittest.TestCase):
    def host(self, stack, value, sources, outputs):
        return stack.enter_context(NativeHost(loan(), json.dumps(value, separators=(",", ":")).encode(),
                                   source_leases=sources, output_leases=outputs,
                                   local_peer="runtime", consent=True))

    def test_source_loss_rejects_pending_call_and_record_before_and_during_http_record(self):
        for index in (0, 1):
            for partial in (False, True):
                with self.subTest(output=index, partial=partial), ExitStack() as stack:
                    value, sources, outputs, _, _ = setup_host()
                    host = self.host(stack, value, sources, outputs)
                    pending = host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)[index]
                    endpoint, spec = unused("http"), pending.spec
                    outgoing = Session("runtime", [Grant("live-send", endpoint, "destination", spec)], consent=True)
                    incoming = Session("destination", [Grant("live-receive", endpoint, "runtime", spec)], consent=True)
                    destination, _ = lease(spec, "owned-null-http", f"output-{index}-epoch")
                    received, seen, lost = [], [], []
                    sequence = iter((pending, None))
                    def read_block():
                        result = next(sequence)
                        if result is not None:
                            seen.append(True)
                            if not partial:
                                host.source_lost(1)
                                lost.append(True)
                        return result
                    original_write = stream_io._Socket.write
                    def partial_write(io, data):
                        # Real socket write of metadata and half the PCM, then
                        # signal loss before the remainder can complete a record.
                        if partial and seen and not lost and len(data) > 32 and data[:1] == b"\0":
                            original_write(io, data[:-16])
                            host.source_lost(1)
                            lost.append(True)
                            return original_write(io, data[-16:])
                        return original_write(io, data)
                    stack.enter_context(patch.object(stream_io._Socket, "write", partial_write))
                    source = stack.enter_context(StreamSource(outgoing, spec, read_block, endpoint=endpoint,
                                                  check_lease=outputs[index].check,
                                                  validate_block=outputs[index].validate))
                    try:
                        StreamReceiver(incoming).receive(source.resource, received.append,
                            accept_block=destination.accept, check_lease=destination.check,
                            block_factory=LeasedBlock)
                    except (PermissionError, ValueError, ConnectionError, TimeoutError):
                        pass  # A rejected/incomplete record must never reach the consumer.
                    self.assertTrue(seen and lost, "loss injection must actually execute")
                    self.assertEqual(received, [], "whole-generation loss must revoke copied Call AND Record PCM")
                    self.assertIsNone(host.stop_witness, "notification is not native scrub completion")

    def test_converted_pending_rtp_rejects_after_actual_cpu_quiesce(self):
        for index in (0, 1):
            with self.subTest(output=index), ExitStack() as stack:
                value, sources, outputs, _, _ = setup_host()
                host = self.host(stack, value, sources, outputs)
                pending = host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)[index]
                target = replace(pending.spec, codec="raw-s24-le")
                converted = convert_processed(pending, target, consent=True, rounding="nearest-ties-even").output
                sender, receiver = udp_pair(stack, target, "runtime", "destination")
                # Independent destination permission must not remove the copied
                # PCM's producing-generation restriction during conversion.
                destination, _ = lease(target, "owned-explicit-s24", f"output-{index}-epoch")
                transmit = LeasedRtpSender(sender, destination, lambda: converted,
                                           **agreement(f"output-{index}-epoch"))
                witness = host.quiesce()
                self.assertIsNotNone(witness)
                received = []
                try:
                    transmit.send()
                except PermissionError:
                    pass
                else:
                    received.append(receiver.receive().pcm)
                self.assertEqual(received, [], "even completed CPU scrub cannot recall an unguarded copy")
                self.assertEqual(sender.operations, 0, "reject before actual RTP send admission")

    def test_source_loss_does_not_revoke_unrelated_host_or_raw_lease(self):
        with ExitStack() as stack:
            value, sources, outputs, _, _ = setup_host()
            lost = self.host(stack, value, sources, outputs)
            lost.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)
            other_value, other_sources, other_outputs, _, _ = setup_host()
            other = self.host(stack, other_value, other_sources, other_outputs)
            pending = other.process(blocks(other_sources), now_ns=1_000_000, deadline_ns=2_000_000)[0]
            raw, _ = lease(pending.spec, "unrelated-raw-owner", "output-0-epoch")
            opaque = replace(pending, pcm=struct.pack("<4Q", 0x8000000000000000,
                             0x7ff8000000001234, 0x7ff0000000000000, 1))
            lost.source_lost(1)
            received = []
            other_outputs[0].accept(pending, received.append)
            raw.accept(opaque, received.append)
            self.assertEqual([item.pcm for item in received], [pending.pcm, opaque.pcm])
            self.assertIsNone(lost.stop_witness)

    def test_native_processing_preserves_inherited_input_stop_restrictions(self):
        from mixyard_interop.runtime import _ProducerReceipt
        with ExitStack() as stack:
            value, sources, outputs, _, _ = setup_host()
            host = self.host(stack, value, sources, outputs)
            stopped = threading.Event()
            incoming = blocks(sources)
            incoming[1] = replace(incoming[1], _producer=_ProducerReceipt(stopped))
            pending = host.process(incoming, now_ns=1_000_000, deadline_ns=2_000_000)
            stopped.set()
            # This is whole bridge-generation loss, even for Call whose PCM
            # excludes return. The contributor metadata remains output-specific.
            for index in (0, 1):
                with self.subTest(output=index), self.assertRaises(PermissionError):
                    outputs[index].accept(pending[index], lambda block: self.fail("stopped input escaped processing"))

    def test_late_upstream_growth_revokes_existing_downstream_converted_rtp_before_upstream_poll(self):
        from mixyard_interop.runtime import _ProducerReceipt
        with ExitStack() as stack:
            av, ass, ao, _, _ = setup_host()
            a = self.host(stack, av, ass, ao)
            first = a.process(blocks(ass), now_ns=1_000_000, deadline_ns=2_000_000)[0]
            bv, bs, bo, _, _ = setup_host()
            bs[2] = lease(first.spec, "owned-source-shared", "output-0-epoch")[0]
            for index, old in enumerate(bo):
                provenance = Provenance(old.spec.provenance.sources, ("fixture", "runtime", "downstream"))
                bo[index] = lease(replace(old.spec, provenance=provenance),
                                  old.binding.observed_id, f"output-{index}-epoch")[0]
            b = stack.enter_context(NativeHost(loan(), json.dumps(bv).encode(), source_leases=bs,
                                              output_leases=bo, local_peer="downstream", consent=True))
            incoming = blocks(bs)
            incoming[2] = first
            copied = b.process(incoming, now_ns=1_000_000, deadline_ns=2_000_000)
            converted = {index: convert_processed(block, replace(block.spec, codec="raw-s24-le"),
                         consent=True, rounding="nearest-ties-even").output for index, block in copied.items()}
            stopped = threading.Event()
            later = blocks(ass, 2)
            later[1] = replace(later[1], _producer=_ProducerReceipt(stopped))
            a.process(later, now_ns=1_050_000, deadline_ns=2_000_000)
            stopped.set()
            # No A-side validation/process/stop is allowed before B's send: the
            # downstream receipt must see the newly added live dependency itself.
            for index in (0, 1):
                with self.subTest(output=index):
                    block = converted[index]
                    sender, receiver = udp_pair(stack, block.spec, "downstream", "destination")
                    dest, _ = lease(block.spec, "owned-conversion", f"output-{index}-epoch")
                    received = []
                    try:
                        LeasedRtpSender(sender, dest, lambda: block,
                                         **agreement(f"output-{index}-epoch")).send()
                    except PermissionError:
                        pass
                    else:
                        received.append(receiver.receive().pcm)
                    self.assertEqual(received, [], "later upstream restrictions must reach existing downstream copies")
                    self.assertEqual(sender.operations, 0)
            with self.assertRaises(PermissionError):
                ao[0].validate(first)
            for index in (0, 1):
                with self.assertRaises(PermissionError):
                    bo[index].validate(copied[index])
            self.assertEqual((a.quiesce().committed_frames, b.quiesce().committed_frames), (4, 2))

    def test_inherited_stop_budget_failure_revokes_existing_processed_copies(self):
        from mixyard_interop.runtime import MAX_PRODUCER_NODES, _ProducerReceipt
        with ExitStack() as stack:
            value, sources, outputs, _, _ = setup_host()
            host = self.host(stack, value, sources, outputs)
            pending = host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)
            inherited = _ProducerReceipt(threading.Event())
            inherited._include(_ProducerReceipt(threading.Event()) for _ in range(MAX_PRODUCER_NODES - 1))
            incoming = blocks(sources, 2)
            incoming[1] = replace(incoming[1], _producer=inherited)
            calls = []
            original_process = host._abi.lib.mx_host_process
            def tracked_process(*args):
                calls.append(True)
                return original_process(*args)
            host._abi.lib.mx_host_process = tracked_process
            # This host's own independent signal would exceed the bound. Fail
            # before processing and revoke previous copies, never truncate it.
            with self.assertRaisesRegex(PermissionError, "stop-dependency node budget"):
                host.process(incoming, now_ns=1_050_000, deadline_ns=2_000_000)
            self.assertEqual(calls, [], "reject the graph before native process work")
            for index in (0, 1):
                with self.assertRaises(PermissionError):
                    outputs[index].validate(pending[index])
            self.assertIsNone(host.stop_witness)
            self.assertEqual(host.quiesce().committed_frames, 2)

    def test_receipt_cycle_rejects_before_native_work_and_preserves_quiesce_ownership(self):
        from mixyard_interop.runtime import _PRODUCER_GRAPH_LOCK, _ProducerReceipt
        with ExitStack() as stack:
            value, sources, outputs, _, _ = setup_host()
            host = self.host(stack, value, sources, outputs)
            calls = []
            original_process = host._abi.lib.mx_host_process
            def tracked_process(*args):
                self.assertFalse(_PRODUCER_GRAPH_LOCK.locked(), "no native callback under receipt graph lock")
                calls.append(True)
                return original_process(*args)
            host._abi.lib.mx_host_process = tracked_process
            pending = host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)
            self.assertEqual(calls, [True])
            calls.clear()
            cycle = _ProducerReceipt(threading.Event())
            cycle._include((host._producer,))
            incoming = blocks(sources, 2)
            incoming[1] = replace(incoming[1], _producer=cycle)
            with self.assertRaisesRegex(PermissionError, "cycle"):
                host.process(incoming, now_ns=1_050_000, deadline_ns=2_000_000)
            self.assertEqual(calls, [])
            for index in (0, 1):
                with self.assertRaises(PermissionError):
                    outputs[index].validate(pending[index])
            self.assertIsNone(host.stop_witness)
            self.assertEqual(host.quiesce().committed_frames, 2)

    def test_actual_plan_contributors_and_float64_dsp_pcm_are_output_specific(self):
        for codec in ("raw-f64-le", "raw-f32-le", "raw-s16-le", "raw-s24-le", "raw-s32-le"):
            with self.subTest(codec=codec), ExitStack() as stack:
                value, sources, outputs, _, _ = setup_host(codec)
                host = self.host(stack, value, sources, outputs)
                if "raw-s" in codec:
                    width = int(codec.split("s")[1].split("-")[0])
                    scale = 2**(width - 1)
                    samples = ((-scale, scale - 1, 1, -1), (scale - 1, -scale, 3, -3))
                    expected = tuple(tuple(v / scale for v in channel) for channel in samples)
                else:
                    samples = ((.5, -.25, .125, -.0625), (.125, -.5, .25, -.125))
                    expected = samples
                result = host.process(blocks(sources, values=samples), now_ns=1_000_000, deadline_ns=2_000_000)
                self.assertEqual(struct.unpack("<4d", result[0].pcm), expected[1])
                self.assertEqual(struct.unpack("<4d", result[1].pcm),
                                 tuple((a * .25 + b * .5) * .5 for a, b in zip(*expected)))
                self.assertEqual([c["sourceIndex"] for c in host.output_proof(0)["contributors"]], [2])
                self.assertEqual({c["sourceIndex"] for c in host.output_proof(1)["contributors"]}, {1, 2})
                self.assertNotIn("owned-source-private", {s.source for b in result.values() for s in b.spec.provenance.sources})
                self.assertEqual(result[0].spec.provenance, outputs[0].spec.provenance)

    def test_unknown_missing_private_provenance_and_clock_mapping_reject(self):
        value, sources, outputs, _, _ = setup_host()
        bad_spec = replace(outputs[0].spec, provenance=outputs[1].spec.provenance)
        wrong, _ = lease(bad_spec, outputs[0].binding.observed_id, "output-0-epoch")
        with ExitStack() as stack, self.assertRaises(NativeUnavailable):
            self.host(stack, value, sources, [wrong, outputs[1]])
        missing = list(sources)
        missing[2] = None
        with ExitStack() as stack, self.assertRaises(NativeUnavailable):
            self.host(stack, value, missing, outputs)
        with ExitStack() as stack:
            host = self.host(stack, value, sources, outputs)
            with self.assertRaises(NativeUnavailable):
                host.process({1: blocks(sources)[1]}, now_ns=1_000_000, deadline_ns=2_000_000)
            with self.assertRaises(PermissionError):
                host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)
        value, sources, outputs, states, _ = setup_host()
        with ExitStack() as stack:
            host = self.host(stack, value, sources, outputs)
            states[2][0] = replace(states[2][0], clock_evidence=replace(
                states[2][0].clock_evidence, base_time_ns=1_000_001))
            with self.assertRaises(PermissionError):
                host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)

    def test_owner_revocation_busy_reentry_restart_and_no_nonfinite_processed_output(self):
        value, sources, outputs, _, _ = setup_host()
        with ExitStack() as stack:
            host = self.host(stack, value, sources, outputs)
            original = sources[1]._owner_check
            def reentrant_owner():
                with self.assertRaises(RuntimeBusy):
                    host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)
                return original()
            sources[1]._owner_check = reentrant_owner
            host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)
            sources[1]._owner_check = original
            host.revoke()
            with self.assertRaises(PermissionError):
                host.process(blocks(sources, 2), now_ns=1_050_000, deadline_ns=2_000_000)
        # A stopped bridge cannot reuse its terminal output leases. A fresh
        # explicit owner admission supplies new leases, instance and frame origin.
        with ExitStack() as stack, self.assertRaises(NativeUnavailable):
            self.host(stack, value, sources, outputs)
        value, sources, outputs, _, _ = setup_host()
        with ExitStack() as stack:
            fresh = self.host(stack, value, sources, outputs)
            fresh.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)
            broken = blocks(sources, 2)
            broken[2] = replace(broken[2], pcm=struct.pack("<4d", float("nan"), 0, 0, 0))
            with self.assertRaises(NativeRejected):
                fresh.process(broken, now_ns=1_050_000, deadline_ns=2_000_000)

    def test_cross_lease_revoke_during_final_owner_callback_discards_packet(self):
        value, sources, outputs, _, _ = setup_host()
        with ExitStack() as stack:
            host = self.host(stack, value, sources, outputs)
            original = outputs[1]._owner_check
            def cross_revoke():
                sources[1].revoke()
                return original()
            outputs[1]._owner_check = cross_revoke
            with self.assertRaises(PermissionError):
                host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)

    def test_source_loss_revokes_whole_generation_but_destination_loss_is_isolated(self):
        value, sources, outputs, _, _ = setup_host()
        with ExitStack() as stack:
            host = self.host(stack, value, sources, outputs)
            host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)
            outputs[1].revoke()
            healthy = host.process(blocks(sources, 2), now_ns=1_050_000, deadline_ns=2_000_000)
            self.assertEqual(set(healthy), {0})
            self.assertIn(1, host.destination_failures)
            host.source_lost(1)
            # Even Call, whose compiled path excludes return, stops with this
            # bridge generation. No per-output native loss-continuation claim.
            with self.assertRaises(PermissionError):
                host.process(blocks(sources, 4), now_ns=1_100_000, deadline_ns=2_000_000)
            self.assertIsNone(host.stop_witness)  # Notification is not completion.
            witness = host.quiesce()
            self.assertEqual((witness.generation, witness.revision, witness.committed_frames), (7, 1, 4))

    def test_failed_quiesce_retains_owner_until_actual_worker_and_native_scrub_complete(self):
        value, sources, outputs, _, _ = setup_host()
        host = NativeHost(loan(), json.dumps(value).encode(), source_leases=sources,
                          output_leases=outputs, local_peer="runtime", consent=True)
        entered, release = threading.Event(), threading.Event()
        original = sources[1]._owner_check
        def held_owner():
            entered.set()
            if not release.wait(1):
                raise TimeoutError("test owner deadline")
            return original()
        sources[1]._owner_check = held_owner
        errors = []
        def run():
            try:
                host.process(blocks(sources), now_ns=1_000_000, deadline_ns=2_000_000)
            except Exception as exc:
                errors.append(exc)
        worker = threading.Thread(target=run)
        worker.start()
        try:
            self.assertTrue(entered.wait(1))
            host.source_lost(1)  # Notification must not wait for the held owner callback.
            with self.assertRaises(RuntimeBusy):
                host.quiesce(timeout_seconds=0)
            self.assertIsNone(host.stop_witness)
            # The caller retains this very context; no new host/rebind occurs.
        finally:
            release.set()
            worker.join(1)
            self.assertFalse(worker.is_alive())
            sources[1]._owner_check = original
            witness = host.quiesce()
            self.assertEqual(witness.committed_frames, 0)
            self.assertEqual(host.close(), witness)
        self.assertEqual(len(errors), 1)
        self.assertIsInstance(errors[0], PermissionError)

    def test_rtp_owned_source_through_actual_host_to_http_owned_destination(self):
        value, sources, outputs, _, _ = setup_host("raw-s24-le")
        values = ((4194304, -2097152, 1048576, -524288), (1048576, -4194304, 2097152, -1048576))
        original = blocks(sources, values=values)
        with ExitStack() as stack:
            host = self.host(stack, value, sources, outputs)
            sender, receiver = udp_pair(stack, sources[2].spec, "fixture", "runtime")
            tx, _ = lease(sources[2].spec, "owned-generator", "source-2-epoch")
            outbound = LeasedRtpSender(sender, tx, lambda: original[2], **agreement("source-2-epoch"))
            processed = []
            def native_sink(received):
                processed.append(host.process({1: original[1], 2: received},
                                               now_ns=1_000_000, deadline_ns=2_000_000)[0])
            inbound = LeasedRtpReceiver(receiver, sources[2], native_sink, timestamp=100,
                                        **agreement("source-2-epoch"))
            outbound.send()
            spec, endpoint = outputs[0].spec, unused("http")
            outgoing = Session("runtime", [Grant("live-send", endpoint, "destination", spec)], consent=True)
            incoming = Session("destination", [Grant("live-receive", endpoint, "runtime", spec)], consent=True)
            reads = []
            def read_processed():
                if reads:
                    return None
                reads.append(1)
                inbound.receive()
                return processed[0]
            stream = stack.enter_context(StreamSource(outgoing, spec, read_processed, endpoint=endpoint,
                                         check_lease=outputs[0].check, validate_block=outputs[0].validate))
            destination, _ = lease(spec, "owned-null-http-consumer", "output-0-epoch")
            received = []
            result = StreamReceiver(incoming).receive(stream.resource,
                     received.append, accept_block=destination.accept, check_lease=destination.check,
                     block_factory=LeasedBlock)
            self.assertEqual((result.blocks, result.frames), (1, 2))
            self.assertEqual(struct.unpack("<4d", received[0].pcm), (.125, -.5, .25, -.125))
            self.assertEqual(received[0].spec.provenance, outputs[0].spec.provenance)
            self.assertEqual(host.output_proof(0)["contributors"][0]["sourceIndex"], 2)

    def test_http_owned_source_through_actual_host_to_explicit_integer_rtp_destination(self):
        value, sources, outputs, _, _ = setup_host("raw-s24-le")
        values = ((4194304, -2097152, 1048576, -524288), (1048576, -4194304, 2097152, -1048576))
        original = blocks(sources, values=values)
        with ExitStack() as stack:
            host = self.host(stack, value, sources, outputs)
            endpoint, spec = unused("http"), sources[2].spec
            outgoing = Session("fixture", [Grant("live-send", endpoint, "runtime", spec)], consent=True)
            incoming = Session("runtime", [Grant("live-receive", endpoint, "fixture", spec)], consent=True)
            tx, _ = lease(spec, "owned-http-generator", "source-2-epoch")
            sequence = iter((original[2], None))
            stream = stack.enter_context(StreamSource(outgoing, spec, lambda: tx.read(lambda: next(sequence)),
                                  endpoint=endpoint, check_lease=tx.check, validate_block=tx.validate))
            processed = []
            def native_sink(received):
                processed.append(host.process({1: original[1], 2: received},
                                               now_ns=1_000_000, deadline_ns=2_000_000)[0])
            StreamReceiver(incoming).receive(stream.resource, native_sink, accept_block=sources[2].accept,
                         check_lease=sources[2].check, block_factory=LeasedBlock)
            target = replace(outputs[0].spec, codec="raw-s24-le")
            evidence = convert_processed(processed[0], target, consent=True, rounding="nearest-ties-even")
            sender, receiver = udp_pair(stack, target, "runtime", "destination")
            converted_tx, state = lease(target, "owned-explicit-s24-conversion", "output-0-epoch")
            # Conversion authority is local to this destination and remains
            # dependent on the original actual Float64 output lease.
            def conversion_owner():
                outputs[0].check()
                return state[0]
            converted_tx._owner_check = conversion_owner
            destination, _ = lease(target, "owned-null-rtp-consumer", "output-0-epoch")
            received = []
            LeasedRtpSender(sender, converted_tx, lambda: evidence.output,
                            **agreement("output-0-epoch")).send()
            LeasedRtpReceiver(receiver, destination, received.append, timestamp=100,
                              **agreement("output-0-epoch")).receive()
            self.assertEqual(received[0].pcm, original[2].pcm)
            self.assertEqual(received[0].spec.provenance, processed[0].spec.provenance)
            self.assertEqual((evidence.source_spec.codec, evidence.rounding),
                             ("raw-f64-le", "nearest-ties-even"))
            self.assertEqual(outputs[0].spec.codec, "raw-f64-le")


if __name__ == "__main__":
    unittest.main()
