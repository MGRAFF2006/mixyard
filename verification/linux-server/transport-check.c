/* SPDX-License-Identifier: MIT
 * Offline check of the real module paths; no daemon, links or audio devices.
 */
#include "../../adapters/linux/server-proof/module.c"
#include <assert.h>
#include <sys/mman.h>

int main(void)
{
    struct proof *p = calloc(1, sizeof(*p));
    assert(p);
    struct node *in = &p->nodes[0], *out = &p->nodes[1];
    struct pw_impl_port port = { .direction = PW_DIRECTION_INPUT, .port_id = 0 };
    port.properties = pw_properties_new(NULL, NULL);
    assert(port.properties);
    spa_hook_list_init(&port.listener_list);
    in->proof = p;
    node_port_init(in, &port);
    assert(port.n_mix == 0 && port.state == PW_IMPL_PORT_STATE_INIT);
    assert((port.flags & PW_IMPL_PORT_FLAG_NO_MIXER) && port.exclusive);
    assert(spa_streq(pw_properties_get(port.properties, PW_KEY_PORT_EXCLUSIVE), "true"));
    remove_hook(&in->port_hook);
    pw_properties_free(port.properties);

    size_t size = (size_t)sysconf(_SC_PAGESIZE);
    float *producer = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    assert(producer != MAP_FAILED);
    producer[0] = .125f;
    producer[1] = -.125f;
    assert(mprotect(producer, size, PROT_READ) == 0);
    struct spa_chunk chunks[4] = {0};
    float output[2] = {.125f, -.125f};
    struct spa_data data_in[2] = {
        { .data = producer, .maxsize = 4, .chunk = &chunks[0] },
        { .data = producer + 1, .maxsize = 4, .chunk = &chunks[1] },
    }, data_out[2] = {
        { .data = output, .maxsize = 4, .chunk = &chunks[2] },
        { .data = output + 1, .maxsize = 4, .chunk = &chunks[3] },
    };
    struct spa_buffer buf_in = { .n_datas = 2, .datas = data_in };
    struct spa_buffer buf_out = { .n_datas = 2, .datas = data_out };
    struct spa_io_buffers io_in = { SPA_STATUS_HAVE_DATA, 0 };
    struct spa_io_buffers io_out = { SPA_STATUS_HAVE_DATA, 0 };
    in->n_buffers = out->n_buffers = 1;
    in->buffers[0] = &buf_in;
    out->buffers[0] = &buf_out;
    in->io = &io_in;
    out->io = &io_out;
    in->started = true;
    in->outstanding[0] = out->outstanding[0] = true;
    p->queue[0].frames = 2;
    p->queue[0].pcm[0][0] = .125f;
    p->write = 1;
    atomic_init(&p->discarded, 0);
    assert(clear_rt(NULL, false, 0, NULL, 0, p) == 0);
    assert(producer[0] == .125f && producer[1] == -.125f);
    assert(output[0] == 0 && output[1] == 0);
    assert(in->io == NULL && !in->started && in->n_buffers == 0);
    for (unsigned i = 0; i < BUFFERS; i++) assert(in->buffers[i] == NULL);
    assert(io_in.buffer_id == SPA_ID_INVALID && io_in.status == SPA_STATUS_NEED_DATA);
    assert(io_out.buffer_id == SPA_ID_INVALID && io_out.status == SPA_STATUS_NEED_DATA);
    assert(!in->outstanding[0] && !out->outstanding[0]);
    assert(p->read == 0 && p->write == 0 && atomic_load(&p->discarded) == 2);
    const unsigned char *queue = (const unsigned char *)p->queue;
    for (size_t i = 0; i < sizeof(p->queue); i++) assert(queue[i] == 0);
    assert(clear_rt(NULL, false, 0, NULL, 0, p) == 0);
    assert(munmap(producer, size) == 0);
    free(p);
    puts("offline transport check: exclusive pre-format ingress, read-only producer unchanged, queue/output cleared, references reset");
    return 0;
}
