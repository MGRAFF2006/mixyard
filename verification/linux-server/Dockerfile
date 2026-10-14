FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
RUN printf 'deb http://deb.debian.org/debian trixie-backports main\n' > /etc/apt/sources.list.d/backports.list \
 && apt-get update \
 && apt-get install -y -t trixie-backports --no-install-recommends gcc libc6-dev make pkg-config python3 meson ninja-build libsndfile1-dev wireplumber=0.5.12-1~bpo13+1 \
 && rm -rf /var/lib/apt/lists/*
RUN useradd -m -u 1000 proof
USER proof
WORKDIR /work
