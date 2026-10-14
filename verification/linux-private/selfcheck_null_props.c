/* SDK-only callback probe: no context, server connection or host graph. */
#define main mixyard_runtime_main
#include "../../adapters/linux/native/mixyard_pw.c"
#undef main
#include <assert.h>

int main(void) {
    struct source source = {.name = "synthetic-null-props", .selected = 42};
    struct object object = {.id = 42, .serial = 123, .name = "synthetic-null-props",
                            .channel = "FL", .node = 42};
    struct app app = {.sources = &source, .ns = 1, .objects = &object};
    struct binding binding = {.app = &app, .id = 42, .serial = 123, .valid = true};
    atomic_init(&source.gate, true);
    atomic_init(&source.epoch, 0);
    struct pw_node_info node = {.id = 42, .change_mask = PW_NODE_CHANGE_MASK_PROPS, .props = NULL};
    node_info(&binding, &node);
    assert(binding.node_info && !binding.node_info->props);
    assert(!binding.valid && source.retired && !atomic_load(&source.gate));
    assert(atomic_load(&source.epoch) == 1);
    pw_node_info_free(binding.node_info);

    source.retired = false;
    atomic_store(&source.gate, true);
    binding.valid = true;
    binding.port = true;
    struct pw_port_info port = {.id = 42, .change_mask = PW_PORT_CHANGE_MASK_PROPS,
                               .direction = PW_DIRECTION_OUTPUT, .props = NULL};
    port_info(&binding, &port);
    assert(binding.port_info && !binding.port_info->props);
    assert(!binding.valid && source.retired && !atomic_load(&source.gate));
    assert(atomic_load(&source.epoch) == 2);
    pw_port_info_free(binding.port_info);
    puts("PASS: null Node/Port properties retire the source without a crash");
    return 0;
}
