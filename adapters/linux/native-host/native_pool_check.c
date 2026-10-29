// SPDX-License-Identifier: MIT
// Actual distro SPA audiomixer ownership counterexample, not an emulated mixer.
// No PipeWire context/server/connection, device, process capture or playback.
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pipewire/pipewire.h>
#include <spa/node/node.h>
#include <spa/node/io.h>
#include <spa/node/command.h>
#include <spa/utils/names.h>
#include "native_format.h"
#include "consumer_copy.h"
#define SAMPLES 256
struct block {
    struct spa_buffer buffer;
    struct spa_meta meta;
    struct spa_data data;
    struct spa_chunk chunk;
    struct spa_meta_header header;
    _Alignas(64) double pcm[SAMPLES];
};
static void init_block(struct block *b, uint64_t frame, double value) {
    memset(b, 0, sizeof(*b));
    for (unsigned i = 0; i < SAMPLES; ++i) b->pcm[i] = value;
    b->header.seq = frame; b->header.pts = (int64_t)frame * 1000;
    b->chunk = (struct spa_chunk){ .size = sizeof(b->pcm), .stride = 2 * sizeof(double) };
    b->data = (struct spa_data){ .type = SPA_DATA_MemPtr, .flags = SPA_DATA_FLAG_READWRITE,
        .maxsize = sizeof(b->pcm), .data = b->pcm, .chunk = &b->chunk };
    b->meta = (struct spa_meta){ .type = SPA_META_Header, .size = sizeof(b->header), .data = &b->header };
    b->buffer = (struct spa_buffer){ .n_metas = 1, .metas = &b->meta, .n_datas = 1, .datas = &b->data };
}
int main(int argc, char **argv) {
    const char *actions = getenv("GITHUB_ACTIONS"), *environment = getenv("RUNNER_ENVIRONMENT");
    if (!actions || strcmp(actions, "true") || !environment || strcmp(environment, "github-hosted")) {
        fputs("ownership probe requires disposable hosted SDK runner; no native execution here\n", stderr);
        return 2;
    }
    pw_init(&argc, &argv);
    struct pw_loop *loop = pw_loop_new(NULL);
    assert(loop);
    pw_loop_enter(loop);
    struct spa_support support[16];
    uint32_t count = pw_get_support(support, 15);
    assert(count < 16);
    support[count++] = SPA_SUPPORT_INIT(SPA_TYPE_INTERFACE_DataLoop, loop->loop);
    struct spa_handle *handle = pw_load_spa_handle("audiomixer/libspa-audiomixer",
        SPA_NAME_AUDIO_MIXER, NULL, count, support);
    assert(handle);
    struct spa_node *node = NULL;
    assert(spa_handle_get_interface(handle, SPA_TYPE_INTERFACE_Node, (void **)&node) == 0 && node);
    assert(spa_node_add_port(node, SPA_DIRECTION_INPUT, 0, NULL) == 0);
    uint8_t storage[2048];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    struct spa_audio_info_raw exact = mx_native_format_requested();
    const struct spa_pod *format = spa_format_audio_raw_build(&builder, SPA_PARAM_Format, &exact);
    assert(format);
    assert(spa_node_port_set_param(node, SPA_DIRECTION_INPUT, 0, SPA_PARAM_Format, 0, format) == 0);
    assert(spa_node_port_set_param(node, SPA_DIRECTION_OUTPUT, 0, SPA_PARAM_Format, 0, format) == 0);
    struct block input[2], output[2];
    init_block(&input[0], 100, 0.25); init_block(&input[1], 228, 0.5);
    init_block(&output[0], 0, 0.0); init_block(&output[1], 0, 0.0);
    struct spa_buffer *inputs[] = { &input[0].buffer, &input[1].buffer };
    struct spa_buffer *outputs[] = { &output[0].buffer, &output[1].buffer };
    assert(spa_node_port_use_buffers(node, SPA_DIRECTION_INPUT, 0, 0, inputs, 2) == 0);
    assert(spa_node_port_use_buffers(node, SPA_DIRECTION_OUTPUT, 0, 0, outputs, 2) == 0);
    struct spa_io_buffers in = { .status = SPA_STATUS_HAVE_DATA, .buffer_id = 0 };
    struct spa_io_buffers out = { .status = SPA_STATUS_NEED_DATA, .buffer_id = SPA_ID_INVALID };
    assert(spa_node_port_set_io(node, SPA_DIRECTION_INPUT, 0, SPA_IO_Buffers, &in, sizeof(in)) == 0);
    assert(spa_node_port_set_io(node, SPA_DIRECTION_OUTPUT, 0, SPA_IO_Buffers, &out, sizeof(out)) == 0);
    struct spa_command start = SPA_NODE_COMMAND_INIT(SPA_NODE_COMMAND_Start);
    assert(spa_node_send_command(node, &start) == 0);
    assert(spa_node_process(node) >= 0 && out.status == SPA_STATUS_HAVE_DATA);
    assert(in.status == SPA_STATUS_NEED_DATA && out.buffer_id < 2);
    struct spa_buffer *held = outputs[out.buffer_id];
    assert(held->datas == input[0].buffer.datas && held->metas == input[0].buffer.metas);
    assert(held->datas[0].chunk == &input[0].chunk && held->datas[0].data == input[0].pcm);
    double copied[SAMPLES]; memcpy(copied, held->datas[0].data, sizeof(copied));
    struct spa_meta_header copied_header = *(struct spa_meta_header *)held->metas[0].data;
    struct mx_consumer_copy owned = {0};
    assert(mx_consumer_open(&owned, 1, 7, 11, 100));
    struct mx_consumer_metadata metadata = { .stamp = { .generation = 7, .revision = 1,
        .clock_id = 11, .captured_ns = 100000, .deadline_ns = 100100000 },
        .consumer_epoch = 1, .native_frame = 100 };
    assert(mx_consumer_push(&owned, 100000, &metadata, held->datas[0].data, SAMPLES) == MX_COPY_OK);
    // Hold both downstream tokens. The plugin nevertheless acknowledges each
    // source input as reusable and aliases its complete buffer into the output.
    out.status = SPA_STATUS_NEED_DATA; out.buffer_id = SPA_ID_INVALID;
    in.status = SPA_STATUS_HAVE_DATA; in.buffer_id = 1;
    assert(spa_node_process(node) >= 0 && in.status == SPA_STATUS_NEED_DATA);
    assert(out.status == SPA_STATUS_HAVE_DATA);
    // The actual input acknowledgement permits upstream reuse. The held output
    // has no independent snapshot of header/chunk/data after that reuse.
    input[0].header.seq = 356; input[0].header.pts = 356000;
    for (unsigned i = 0; i < SAMPLES; ++i) input[0].pcm[i] = 0.875;
    assert(((struct spa_meta_header *)held->metas[0].data)->seq == 356);
    assert(((double *)held->datas[0].data)[0] == 0.875);
    assert(copied_header.seq == 100);
    for (unsigned i = 0; i < SAMPLES; ++i) assert(copied[i] == 0.25);
    const struct mx_consumer_packet *owned_packet = NULL;
    assert(mx_consumer_front(&owned, 1, 7, 11, 100001, &owned_packet) == MX_COPY_OK);
    assert(owned_packet->metadata.native_frame == 100);
    for (unsigned i = 0; i < SAMPLES; ++i) assert(owned_packet->pcm[i] == 0.25);
    mx_consumer_close(&owned);
    out.status = SPA_STATUS_NEED_DATA; out.buffer_id = SPA_ID_INVALID;
    in.status = SPA_STATUS_HAVE_DATA; in.buffer_id = 0;
    int exhausted = spa_node_process(node);
    assert(exhausted == -EPIPE && in.status == SPA_STATUS_NEED_DATA);
    // This expected counterexample is diagnostic evidence, never a graph PASS.
    printf("{\"actualSpaPlugin\":true,\"singleInputBufferAlias\":true,\"heldMetadataChanged\":true,\"heldPcmChanged\":true,\"ownedCopyStable\":true,\"ownedConsumerCopyStable\":true,\"poolBuffers\":2,\"exhaustedResult\":%d}\n", exhausted);
    assert(pw_unload_spa_handle(handle) == 0);
    pw_loop_leave(loop); pw_loop_destroy(loop); pw_deinit();
    return 0;
}
