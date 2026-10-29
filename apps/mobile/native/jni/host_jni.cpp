#include <jni.h>
#include <mixyard_host.h>
#include <cstring>
#include <memory>
#include <vector>

// Exclusive non-realtime worker attachment. No OS/audio/TLS or policy model.
// PCM pointers always reference direct buffers retained by the Kotlin owner.
namespace {
struct Worker {
    mx_host* host = nullptr;
    std::vector<mx_input> inputs;
    std::vector<uint8_t*> outputs;
    std::vector<size_t> output_sizes;
    uint8_t* stamps = nullptr;
    size_t stamp_bytes = 0;
    ~Worker() { if (host) mx_host_destroy(host); }
};
Worker* worker(jlong handle) { return reinterpret_cast<Worker*>(handle); }
bool direct(JNIEnv* env, jobject buffer, uint8_t*& bytes, size_t& size) {
    if (!buffer) return false;
    const auto capacity = env->GetDirectBufferCapacity(buffer);
    bytes = static_cast<uint8_t*>(env->GetDirectBufferAddress(buffer));
    if (capacity <= 0 || !bytes) return false;
    size = static_cast<size_t>(capacity); return true;
}
void scrub(Worker* value) {
    if (!value) return;
    mx_host_revoke(value->host);
    for (const auto& input : value->inputs)
        if (input.bytes) std::memset(const_cast<uint8_t*>(input.bytes), 0, input.byte_count);
    for (size_t i = 0; i < value->outputs.size(); ++i)
        std::memset(value->outputs[i], 0, value->output_sizes[i]);
}
}
extern "C" JNIEXPORT jlong JNICALL
Java_org_mixyard_mobile_host_NativeHostWorker_create(JNIEnv* env, jobject, jbyteArray request) {
    if (!request) return 0;
    const auto size = env->GetArrayLength(request);
    if (size <= 0 || size > 1048576) return 0;
    std::vector<uint8_t> bytes(static_cast<size_t>(size));
    env->GetByteArrayRegion(request, 0, size, reinterpret_cast<jbyte*>(bytes.data()));
    if (env->ExceptionCheck()) return 0;
    auto value = std::make_unique<Worker>();
    char error[512] = {};
    if (mx_host_create(bytes.data(), bytes.size(), &value->host, error, sizeof(error)) != MX_OK) return 0;
    value->inputs.resize(mx_host_source_count(value->host));
    value->outputs.resize(mx_host_output_count(value->host));
    value->output_sizes.resize(value->outputs.size());
    return reinterpret_cast<jlong>(value.release());
}
extern "C" JNIEXPORT jstring JNICALL
Java_org_mixyard_mobile_host_NativeHostWorker_info(JNIEnv* env, jobject, jlong handle, jboolean output, jint index) {
    auto* value = worker(handle);
    if (!value || index < 0) return nullptr;
    const auto get = output ? mx_host_output_info : mx_host_source_info;
    size_t needed = 0;
    if (get(value->host, static_cast<size_t>(index), nullptr, 0, &needed) != MX_BUFFER || needed > 1048576) return nullptr;
    std::vector<uint8_t> bytes(needed + 1, 0);
    if (get(value->host, static_cast<size_t>(index), bytes.data(), needed, &needed) != MX_OK) return nullptr;
    // Port JSON is returned as UTF-8 bytes elsewhere; JNI modified UTF-8 is not
    // valid for arbitrary channel/observed IDs, so never use NewStringUTF.
    const auto array = env->NewByteArray(static_cast<jsize>(needed));
    if (!array) return nullptr;
    env->SetByteArrayRegion(array, 0, static_cast<jsize>(needed), reinterpret_cast<const jbyte*>(bytes.data()));
    const auto string_class = env->FindClass("java/lang/String");
    if (!string_class) return nullptr;
    const auto constructor = env->GetMethodID(string_class, "<init>", "([BLjava/lang/String;)V");
    if (!constructor) return nullptr;
    const auto charset = env->NewStringUTF("UTF-8");
    if (!charset) return nullptr;
    return static_cast<jstring>(env->NewObject(string_class, constructor, array, charset));
}
extern "C" JNIEXPORT jint JNICALL
Java_org_mixyard_mobile_host_NativeHostWorker_bindInput(JNIEnv* env, jobject, jlong handle, jint index, jobject buffer, jint encoding, jint rate, jint channels) {
    auto* value = worker(handle);
    if (!value || index < 0 || static_cast<size_t>(index) >= value->inputs.size() || rate == 0 || channels <= 0) return MX_INVALID;
    uint8_t* bytes = nullptr; size_t size = 0;
    if (!direct(env, buffer, bytes, size)) return MX_BUFFER;
    auto& input = value->inputs[static_cast<size_t>(index)];
    input.bytes = bytes; input.byte_count = size; input.encoding = static_cast<uint32_t>(encoding);
    input.sample_rate_hz = static_cast<uint32_t>(rate); input.channels = static_cast<uint32_t>(channels);
    return MX_OK;
}
extern "C" JNIEXPORT jint JNICALL
Java_org_mixyard_mobile_host_NativeHostWorker_bindOutput(JNIEnv* env, jobject, jlong handle, jint index, jobject buffer) {
    auto* value = worker(handle);
    if (!value || index < 0 || static_cast<size_t>(index) >= value->outputs.size()) return MX_INVALID;
    return direct(env, buffer, value->outputs[static_cast<size_t>(index)], value->output_sizes[static_cast<size_t>(index)]) ? MX_OK : MX_BUFFER;
}
extern "C" JNIEXPORT jint JNICALL
Java_org_mixyard_mobile_host_NativeHostWorker_bindStamps(JNIEnv* env, jobject, jlong handle, jobject buffer) {
    auto* value = worker(handle);
    if (!value || !direct(env, buffer, value->stamps, value->stamp_bytes)) return MX_BUFFER;
    return value->stamp_bytes == (value->inputs.size() + 1) * sizeof(mx_stamp) ? MX_OK : MX_BUFFER;
}
extern "C" JNIEXPORT jint JNICALL
Java_org_mixyard_mobile_host_NativeHostWorker_process(JNIEnv*, jobject, jlong handle, jlong now, jint frames) {
    auto* value = worker(handle);
    if (!value) return MX_INVALID;
    if (now < 0 || frames <= 0 || !value->stamps || value->stamp_bytes != (value->inputs.size() + 1) * sizeof(mx_stamp)) { scrub(value); return MX_INVALID; }
    static_assert(sizeof(mx_stamp) == 7 * sizeof(uint64_t), "ABI stamp layout changed");
    mx_stamp stamp;
    std::memcpy(&stamp, value->stamps, sizeof(stamp));
    for (size_t i = 0; i < value->inputs.size(); ++i) {
        auto& input = value->inputs[i];
        if (!input.bytes) { scrub(value); return MX_BUFFER; }
        std::memcpy(&input.stamp, value->stamps + (i + 1) * sizeof(mx_stamp), sizeof(mx_stamp));
    }
    const auto result = mx_host_process(value->host, static_cast<uint64_t>(now), stamp, static_cast<size_t>(frames), value->inputs.data(), value->inputs.size());
    if (result != MX_OK) { scrub(value); return result; }
    for (size_t i = 0; i < value->outputs.size(); ++i) {
        const double* samples = nullptr; size_t count = 0;
        const auto status = mx_host_output(value->host, i, &samples, &count);
        if (status != MX_OK || !samples || count != value->output_sizes[i] / sizeof(double) || !value->outputs[i]) { scrub(value); return MX_BUFFER; }
        std::memset(value->outputs[i], 0, value->output_sizes[i]);
        std::memcpy(value->outputs[i], samples, count * sizeof(double));
    }
    return MX_OK;
}
extern "C" JNIEXPORT void JNICALL
Java_org_mixyard_mobile_host_NativeHostWorker_revoke(JNIEnv*, jobject, jlong handle) { scrub(worker(handle)); }
extern "C" JNIEXPORT void JNICALL
Java_org_mixyard_mobile_host_NativeHostWorker_destroy(JNIEnv*, jobject, jlong handle) { delete worker(handle); }
