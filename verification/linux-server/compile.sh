#!/usr/bin/env bash
set -euo pipefail
base=/work/verification/linux-server/build
cd "$base/source"
if [[ ! -f build/build.ninja ]]; then
    meson setup build --prefix="$base/install" --libdir=lib -Dauto_features=disabled \
      -Ddbus=disabled -Dsession-managers=[] -Dtests=disabled -Dexamples=disabled \
      -Dpipewire-jack=disabled -Dpipewire-v4l2=disabled -Dflatpak=disabled \
      -Dspa-plugins=enabled -Dsupport=enabled -Daudioconvert=enabled \
      -Daudiomixer=enabled -Daudiotestsrc=enabled -Dsndfile=enabled -Dpw-cat=enabled
fi
ninja -C build -j2
meson install -C build --no-rebuild
dpkg-query -W > "$base/packages.tsv"
meson configure build > "$base/meson-config.txt"
"$base/install/bin/pipewire" --version > "$base/version.txt"
if [[ -f /work/adapters/linux/server-proof/module.c ]]; then
    mkdir -p "$base/modules"
    bash /work/adapters/linux/server-proof/compile.sh "$base/source" "$base/modules"
fi
if [[ -f /work/verification/linux-server/fixture.c ]]; then
    cc -std=gnu11 -D_GNU_SOURCE -O2 -Wall -Wextra -Werror \
      -Wno-unused-parameter -Wno-missing-field-initializers \
      -I"$base/source/src" -I"$base/source/spa/include" \
      -I"$base/source/build" -I"$base/source/build/src" \
      /work/verification/linux-server/fixture.c -o "$base/fixture" \
      -L"$base/install/lib" -lpipewire-0.3 -lm
fi
cc -std=gnu11 -D_GNU_SOURCE -O2 -Wall -Wextra -Werror \
  -Wno-unused-parameter -Wno-missing-field-initializers \
  -I"$base/source/src" -I"$base/source/spa/include" \
  -I"$base/source/build" -I"$base/source/build/src" \
  /work/verification/linux-server/transport-check.c -o "$base/transport-check" \
  -L"$base/install/lib" -lpipewire-0.3
LD_LIBRARY_PATH="$base/install/lib" "$base/transport-check"
