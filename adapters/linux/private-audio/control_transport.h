/* SPDX-License-Identifier: MIT */
#ifndef MIXYARD_PRIVATE_CONTROL_TRANSPORT_H
#define MIXYARD_PRIVATE_CONTROL_TRANSPORT_H
#include <fcntl.h>
#include <stdbool.h>
#include <stddef.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

/* Exact launcher contract: the anonymous connected UnixStream socketpair end
 * duplicated to fd3. No FIFO, datagram, pathname socket or arbitrary
 * descriptor. This is transport identity only; every command still binds
 * nonce/epoch. */
static inline bool mx_private_cancel_socket(int fd, pid_t parent) {
  struct stat st;
  int type = 0, flags = fcntl(fd, F_GETFL);
  socklen_t size = sizeof(type);
  struct sockaddr_un local = {0}, peer = {0};
  socklen_t local_size = sizeof(local), peer_size = sizeof(peer);
  struct ucred credentials;
  socklen_t credential_size = sizeof(credentials);
  return fd > 2 && parent > 0 && !fstat(fd, &st) && S_ISSOCK(st.st_mode) &&
         flags >= 0 && (flags & O_ACCMODE) == O_RDWR &&
         !getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &size) &&
         size == sizeof(type) && type == SOCK_STREAM &&
         !getsockname(fd, (struct sockaddr *)&local, &local_size) &&
         !getpeername(fd, (struct sockaddr *)&peer, &peer_size) &&
         local.sun_family == AF_UNIX && peer.sun_family == AF_UNIX &&
         local_size == offsetof(struct sockaddr_un, sun_path) &&
         peer_size == offsetof(struct sockaddr_un, sun_path) &&
         !getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &credentials,
                     &credential_size) &&
         credential_size == sizeof(credentials) &&
         credentials.uid == geteuid() && credentials.pid == parent;
}
#endif
