// SPDX-License-Identifier: MIT
// Test-only public registry observer. Query only Format, not every advertised
// parameter: standard 1.4.8 adapter nodes reject ProcessLatency enumeration.
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <json-c/json.h>
#define MAX_OBJECTS 256
struct app;
struct item {
    struct app *app;
    uint32_t id;
    json_object *object;
    struct pw_proxy *proxy;
    struct spa_hook hook;
    bool queried;
};
struct app {
    struct pw_main_loop *loop;
    struct pw_context *context;
    struct pw_core *core;
    struct pw_registry *registry;
    struct spa_hook core_hook, registry_hook;
    struct item items[MAX_OBJECTS];
    size_t count;
    int sync, rounds, error;
};
static void fail(struct app *a, const char *why) {
    if (!a->error) fprintf(stderr, "registry observer: %s\n", why);
    a->error = 1; pw_main_loop_quit(a->loop);
}
static json_object *info(struct item *item) {
    json_object *value;
    if (!json_object_object_get_ex(item->object, "info", &value)) {
        value = json_object_new_object(); json_object_object_add(item->object, "info", value);
    }
    return value;
}
static void properties(struct item *item, const struct spa_dict *dict) {
    json_object *props;
    if (!json_object_object_get_ex(info(item), "props", &props)) {
        props = json_object_new_object(); json_object_object_add(info(item), "props", props);
    }
    if (!dict) return;
    const struct spa_dict_item *entry;
    spa_dict_for_each(entry, dict) {
        if (strlen(entry->key) > 256 || strlen(entry->value) > 4096) { fail(item->app, "property bound"); return; }
        json_object_object_add(props, entry->key, json_object_new_string(entry->value));
    }
}
static struct item *get(struct app *a, uint32_t id, const char *type) {
    for (size_t i = 0; i < a->count; ++i) if (a->items[i].id == id) return &a->items[i];
    if (a->count == MAX_OBJECTS) { fail(a, "object bound"); return NULL; }
    struct item *item = &a->items[a->count++];
    item->app = a; item->id = id; item->object = json_object_new_object();
    json_object_object_add(item->object, "id", json_object_new_int64(id));
    json_object_object_add(item->object, "type", json_object_new_string(type));
    return item;
}
static void core_info(void *data, const struct pw_core_info *value) {
    struct app *a = data; struct item *item = get(a, value->id, PW_TYPE_INTERFACE_Core);
    if (item) properties(item, value->props);
}
static void core_error(void *data, uint32_t id, int seq, int result, const char *message) {
    (void)id; (void)seq; (void)result; (void)message;
    fail(data, "unexpected public API core error");
}
static void done(void *data, uint32_t id, int seq) {
    (void)id; struct app *a = data;
    if (seq != a->sync) return;
    if (++a->rounds < 3) a->sync = pw_core_sync(a->core, PW_ID_CORE, 0);
    else pw_main_loop_quit(a->loop);
}
static const struct pw_core_events core_events = { PW_VERSION_CORE_EVENTS, .info = core_info, .done = done, .error = core_error };
static void node_info(void *data, const struct pw_node_info *value) { properties(data, value->props); }
static const struct pw_node_events node_events = { PW_VERSION_NODE_EVENTS, .info = node_info };
static void client_info(void *data, const struct pw_client_info *value) { properties(data, value->props); }
static const struct pw_client_events client_events = { PW_VERSION_CLIENT_EVENTS, .info = client_info };
static void port_info(void *data, const struct pw_port_info *value) {
    struct item *item = data; properties(item, value->props);
    bool readable = false;
    for (uint32_t i = 0; i < value->n_params; ++i)
        if (value->params[i].id == SPA_PARAM_Format && (value->params[i].flags & SPA_PARAM_INFO_READ)) readable = true;
    if (!item->queried && readable) {
        item->queried = true;
        if (pw_port_enum_params((struct pw_port *)item->proxy, 0, SPA_PARAM_Format, 0, 1, NULL) < 0)
            fail(item->app, "Format enumeration request rejected");
    }
}
static void port_param(void *data, int seq, uint32_t id, uint32_t index, uint32_t next, const struct spa_pod *param) {
    (void)seq; (void)index; (void)next;
    struct item *item = data; struct spa_audio_info_raw raw = {0};
    if (id != SPA_PARAM_Format || !param) return;
    if (spa_format_audio_raw_parse(param, &raw) < 0 || raw.channels != 2) { fail(item->app, "unknown/non-stereo raw format"); return; }
    json_object *fmt = json_object_new_object(), *channels = json_object_new_array();
    json_object_object_add(fmt, "format", json_object_new_int(raw.format));
    json_object_object_add(fmt, "rate", json_object_new_int(raw.rate));
    json_object_object_add(fmt, "channels", json_object_new_int(raw.channels));
    for (uint32_t i = 0; i < raw.channels; ++i) json_object_array_add(channels, json_object_new_int(raw.position[i]));
    json_object_object_add(fmt, "positions", channels);
    json_object_object_add(info(item), "negotiatedFormat", fmt);
}
static const struct pw_port_events port_events = { PW_VERSION_PORT_EVENTS, .info = port_info, .param = port_param };
static void link_info(void *data, const struct pw_link_info *value) {
    struct item *item = data; properties(item, value->props); json_object *i = info(item);
    json_object_object_add(i, "output-node-id", json_object_new_int64(value->output_node_id));
    json_object_object_add(i, "output-port-id", json_object_new_int64(value->output_port_id));
    json_object_object_add(i, "input-node-id", json_object_new_int64(value->input_node_id));
    json_object_object_add(i, "input-port-id", json_object_new_int64(value->input_port_id));
}
static const struct pw_link_events link_events = { PW_VERSION_LINK_EVENTS, .info = link_info };
static void added(void *data, uint32_t id, uint32_t permissions, const char *type, uint32_t version, const struct spa_dict *props) {
    (void)permissions; struct app *a = data; struct item *item = get(a, id, type);
    if (!item) return;
    properties(item, props);
    if (!strcmp(type, PW_TYPE_INTERFACE_Link)) {
        const char *keys[] = { PW_KEY_LINK_OUTPUT_NODE, PW_KEY_LINK_OUTPUT_PORT, PW_KEY_LINK_INPUT_NODE, PW_KEY_LINK_INPUT_PORT };
        const char *names[] = { "output-node-id", "output-port-id", "input-node-id", "input-port-id" };
        for (size_t i = 0; i < 4; ++i) {
            const char *text = props ? spa_dict_lookup(props, keys[i]) : NULL;
            if (!text || !*text || *text == '-') continue;
            errno = 0; char *end = NULL; unsigned long long value = strtoull(text, &end, 10);
            if (errno || !end || *end || value > UINT32_MAX) { fail(a, "invalid global link identity"); return; }
            json_object_object_add(info(item), names[i], json_object_new_int64(value));
        }
    }
    if (!strcmp(type, PW_TYPE_INTERFACE_Node)) {
        item->proxy = pw_registry_bind(a->registry, id, type, SPA_MIN(version, (uint32_t)PW_VERSION_NODE), 0);
        if (item->proxy) pw_node_add_listener((struct pw_node *)item->proxy, &item->hook, &node_events, item);
    } else if (!strcmp(type, PW_TYPE_INTERFACE_Client)) {
        item->proxy = pw_registry_bind(a->registry, id, type, SPA_MIN(version, (uint32_t)PW_VERSION_CLIENT), 0);
        if (item->proxy) pw_client_add_listener((struct pw_client *)item->proxy, &item->hook, &client_events, item);
    } else if (!strcmp(type, PW_TYPE_INTERFACE_Port)) {
        item->proxy = pw_registry_bind(a->registry, id, type, SPA_MIN(version, (uint32_t)PW_VERSION_PORT), 0);
        if (item->proxy) pw_port_add_listener((struct pw_port *)item->proxy, &item->hook, &port_events, item);
    } else if (!strcmp(type, PW_TYPE_INTERFACE_Link)) {
        item->proxy = pw_registry_bind(a->registry, id, type, SPA_MIN(version, (uint32_t)PW_VERSION_LINK), 0);
        if (item->proxy) pw_link_add_listener((struct pw_link *)item->proxy, &item->hook, &link_events, item);
    }
}
static const struct pw_registry_events registry_events = { PW_VERSION_REGISTRY_EVENTS, .global = added };
static void timeout(void *data, uint64_t expirations) { (void)expirations; fail(data, "snapshot deadline"); }
int main(int argc, char **argv) {
    if (argc != 1 || !getenv("MIXYARD_PRIVATE_GRAPH") || strcmp(getenv("MIXYARD_PRIVATE_GRAPH"), "hosted-owned-only")) return 2;
    pw_init(&argc, &argv); struct app a = {0}; a.loop = pw_main_loop_new(NULL);
    if (!a.loop) return 2;
    a.context = pw_context_new(pw_main_loop_get_loop(a.loop), NULL, 0);
    if (!a.context) return 2;
    a.core = pw_context_connect(a.context, pw_properties_new(PW_KEY_REMOTE_NAME, "mixyard-private", NULL), 0);
    if (!a.core) return 2;
    pw_core_add_listener(a.core, &a.core_hook, &core_events, &a);
    a.registry = pw_core_get_registry(a.core, PW_VERSION_REGISTRY, 0);
    pw_registry_add_listener(a.registry, &a.registry_hook, &registry_events, &a);
    struct spa_source *timer = pw_loop_add_timer(pw_main_loop_get_loop(a.loop), timeout, &a);
    struct timespec deadline = { .tv_sec = 3 };
    pw_loop_update_timer(pw_main_loop_get_loop(a.loop), timer, &deadline, NULL, false);
    a.sync = pw_core_sync(a.core, PW_ID_CORE, 0); pw_main_loop_run(a.loop);
    if (!a.error) {
        json_object *array = json_object_new_array();
        for (size_t i = 0; i < a.count; ++i) json_object_array_add(array, json_object_get(a.items[i].object));
        const char *text = json_object_to_json_string_ext(array, JSON_C_TO_STRING_PLAIN);
        if (strlen(text) > 1024 * 1024) a.error = 1; else puts(text);
        json_object_put(array);
    }
    for (size_t i = 0; i < a.count; ++i) {
        if (a.items[i].proxy) pw_proxy_destroy(a.items[i].proxy);
        json_object_put(a.items[i].object);
    }
    pw_proxy_destroy((struct pw_proxy *)a.registry); pw_core_disconnect(a.core);
    pw_context_destroy(a.context); pw_main_loop_destroy(a.loop); pw_deinit(); return a.error ? 1 : 0;
}
