FROM ubuntu:24.04

# SDK and isolated display only. No host display/audio sockets or devices.
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl pkg-config libwebkit2gtk-4.1-dev \
    librsvg2-dev libayatana-appindicator3-dev webkit2gtk-driver \
    xvfb xauth xdotool openbox dbus-x11 python3 fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*
