package org.mixyard.mobile.host;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Files;
import java.nio.file.Path;

/** JNI ABI entrypoints only, not a replacement Kotlin worker/policy model.
 * Run with the existing JDK's source-file launcher; no jars/SDK downloads. */
public final class NativeHostWorker {
    private native long create(byte[] request);
    private native String info(long handle, boolean output, int index);
    private native int bindInput(long handle, int index, ByteBuffer bytes, int encoding, int rate, int channels);
    private native int bindOutput(long handle, int index, ByteBuffer bytes);
    private native int bindStamps(long handle, ByteBuffer stamps);
    private native int process(long handle, long nowNs, int frames);
    private native void revoke(long handle);
    private native void destroy(long handle);
    private static void require(boolean value, String label) {
        if (!value) throw new AssertionError(label);
    }
    private static ByteBuffer buffer(int size) {
        return ByteBuffer.allocateDirect(size).order(ByteOrder.LITTLE_ENDIAN);
    }
    public static void main(String[] args) throws Exception {
        require(args.length == 2, "native library and owned fixture required");
        System.load(Path.of(args[0]).toAbsolutePath().toString());
        var api = new NativeHostWorker();
        long handle = api.create(Files.readAllBytes(Path.of(args[1])));
        require(handle != 0, "actual host admission");
        try {
            int sources = 3, outputs = 2, frames = 4;
            var input = new ByteBuffer[sources];
            var output = new ByteBuffer[outputs];
            var stamps = buffer(56 * (sources + 1));
            require(api.info(handle, false, sources) == null && api.info(handle, true, outputs) == null, "exact port count");
            for (int i = 0; i < sources; i++) {
                String info = api.info(handle, false, i);
                require(info != null && info.contains("\"sampleRateHz\":48000"), "actual source info");
                input[i] = buffer(frames * 2 * 8);
                double level = info.contains("owned-source-private") ? 999 : info.contains("owned-source-return") ? 4 : 2;
                for (int sample = 0; sample < frames * 2; sample++) input[i].putDouble(sample * 8, level);
                require(api.bindInput(handle, i, input[i], 2, 48000, 2) == 0, "bind input");
            }
            for (int i = 0; i < outputs; i++) {
                output[i] = buffer(frames * 2 * 8);
                require(api.bindOutput(handle, i, output[i]) == 0, "bind output");
            }
            for (int i = 0; i <= sources; i++) {
                int offset = i * 56;
                stamps.putLong(offset, 7); stamps.putLong(offset + 8, 1); stamps.putLong(offset + 16, 11);
                stamps.putLong(offset + 24, 0); stamps.putLong(offset + 32, 0);
                stamps.putLong(offset + 40, 100); stamps.putLong(offset + 48, 1_000_000);
            }
            require(api.bindStamps(handle, stamps) == 0, "bind stamps");
            require(api.process(handle, 200, frames) == 0, "actual JNI to canonical Rust process");
            for (int i = 0; i < outputs; i++) {
                String info = api.info(handle, true, i);
                double expected = info.contains("owned-consumer-call") ? 2 : 1;
                for (int sample = 0; sample < frames * 2; sample++)
                    require(output[i].getDouble(sample * 8) == expected, "canonical policy and independent DSP output");
            }
            api.revoke(handle);
            for (var bytes : input) for (int i = 0; i < bytes.capacity(); i++) require(bytes.get(i) == 0, "revoked staged input scrub");
            for (var bytes : output) for (int i = 0; i < bytes.capacity(); i++) require(bytes.get(i) == 0, "revoked output scrub");
            require(api.process(handle, 300, frames) != 0, "revoked graph cannot resume");
            System.out.println("PASS actual JVM/JNI/Rust policy, independent DSP and input/output revoke scrubbing; Kotlin worker and physical audio not proved by this JNI probe");
        } finally { api.destroy(handle); }
    }
}
