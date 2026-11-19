# The build environment for Ferrite: the pinned nightly, the ARM binutils and Python, nothing installed on the host.
# Used through compose.yaml and tools/docker.sh (DEVELOPING.md, "Build in Docker"). The repo is mounted at /src, not
# copied in: the build writes target/ on the host as a local build would.
FROM debian:bookworm-slim

# The same packages as the release workflow (.forgejo/workflows/release.yml), plus what its node image already had:
# python3 (the tools, stdlib only); binutils-arm-none-eabi (slowcheck, the size log); the X11/Wayland headers
# (the simulator's window library, minifb, builds against them even headless: mkbtf.sh runs ferrite-mkparts);
# gcc and libc6-dev (build scripts, the host linker); git (the build ID's commit, the size log); perl (shasum).
RUN apt-get update -q \
 && apt-get install -y -q --no-install-recommends \
      ca-certificates curl gcc libc6-dev git perl python3 binutils-arm-none-eabi pkg-config \
      libx11-dev libxkbcommon-dev libwayland-dev libxcursor-dev libxrandr-dev libxi-dev \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# The nightly rust-toolchain.toml pins, with its components and targets, baked into the image so a build doesn't
# fetch it. Changing rust-toolchain.toml rebuilds this layer (tools/docker.sh builds the image when it changes).
# The container runs as the host's user (tools/docker.sh), so target/ stays the owner's: any uid may use the
# toolchain and fill the crate caches (named volumes, which take these permissions when first created).
COPY rust-toolchain.toml /tmp/toolchain/
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain none \
 && cd /tmp/toolchain && rustup toolchain install \
 && rm -rf /tmp/toolchain \
 && mkdir -p "$CARGO_HOME/registry" "$CARGO_HOME/git" \
 && chmod -R a+rwX "$RUSTUP_HOME" "$CARGO_HOME"

# An arbitrary uid has no home and doesn't own the mounted repo in git's eyes.
ENV HOME=/tmp \
    GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0=* \
    CARGO_TERM_COLOR=always

WORKDIR /src
CMD ["tools/mkbtf.sh", "tx"]
