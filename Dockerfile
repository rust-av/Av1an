FROM archlinux:base-devel AS base

RUN pacman -Syu --noconfirm

# Install dependencies needed by all steps including runtime step
RUN pacman -S --noconfirm --needed python python-pip ffms2 ffmpeg mkvtoolnix-cli aom svt-av1 rav1e libvpx vmaf
# Install Python runtime dependencies system-wide so they are available to the app
COPY .github/docker-requirements.txt /tmp/docker-requirements.txt
RUN python -m pip --isolated install --no-cache-dir --break-system-packages \
    --index-url https://pypi.org/simple --only-binary=:all: -r /tmp/docker-requirements.txt && \
    rm /tmp/docker-requirements.txt

# Add extra plugins to ENV to cover VS R74 packaging changes
ENV VAPOURSYNTH_EXTRA_PLUGIN_PATH="/usr/lib/vapoursynth"
# Both metric libraries are built in the `metrics` stage and staged in /usr/lib.
# VSHIP_PLUGIN_PATH is left unset: no VapourSynth plugin is installed here, so an
# override would only name an empty directory.
ENV VSHIP_LIB_DIR="/usr/lib"
# Where libvmaf looks for the VMAF model files
ENV VMAF_MODEL_PATH="/usr/share/model"
# fmetrics is the CPU engine, and the only one available without a GPU
ENV FMETRICS_LIB_DIR="/usr/lib"

# Install ZooMVTools with generic linux binary
COPY .github/scripts/download-verified.sh /usr/local/bin/download-verified.sh
RUN sed -i 's/\r$//' /usr/local/bin/download-verified.sh && \
    ZOOMVTOOLS_VERSION="v2.0.2" && \
    PLUGIN_DIR="$(python -c 'import site; print(site.getsitepackages()[0])')/vapoursynth/plugins" && \
    mkdir -p "$PLUGIN_DIR" && \
    bash /usr/local/bin/download-verified.sh \
    c3446598c810eaa55334b6238de134d9f3e3041e5cf3e44775249f01d00d4d56 \
    "$PLUGIN_DIR/libzoomvtools.so" \
    "https://gitlab.com/api/v4/projects/78027771/packages/generic/vapoursynth-zoomvtools/${ZOOMVTOOLS_VERSION}/vapoursynth-zoomvtools-${ZOOMVTOOLS_VERSION}-linux-x86_64.so"

FROM base AS build-base

# Install dependencies needed by build steps
RUN pacman -S --noconfirm --needed git clang vapoursynth rust nasm

RUN cargo install cargo-chef
WORKDIR /tmp/Condor


FROM build-base AS planner

COPY . .
RUN cargo chef prepare


FROM build-base AS build

COPY --from=planner /tmp/Condor/recipe.json recipe.json
RUN cargo chef cook --release

# Build Condor California
COPY . /tmp/Condor

RUN cargo build --release -p california-condor && \
    mv ./target/release/av1an /usr/local/bin && \
    cd .. && rm -rf ./Condor


# Builds libvship and fmetrics from source; neither is packaged for Linux.
# Kept apart from `build` so a Condor change does not invalidate them.
FROM base AS metrics

# git, pinned Zig, clang and vulkan-headers are build-time only. vulkan-icd-loader is
# also a runtime dependency, installed again below.
#
# Vship 5.1.2 uses lazy Vulkan initialization so the library can load without
# a driver; fmetrics remains the CPU fallback for scoring without a GPU.
RUN pacman -S --noconfirm --needed \
    git \
    clang \
    vulkan-headers \
    vulkan-icd-loader

# Both installers are standalone, and their pins come from .github/.env -- the
# same file the release-staging scripts read, so the image cannot drift from it.
COPY .github/.env /tmp/cicd.env
COPY av-metrics-vship/scripts/install-libvship-linux.sh av-metrics-fmetrics/scripts/install-fmetrics-linux.sh /usr/local/bin/

# Run via bash: the scripts are not executable and may carry CRLF. Sourcing the
# env file exports every pin for the installers to pick up. The env file may
# also carry CRLF (Windows checkout), so strip it before sourcing.
RUN set -eu && \
    sed -i 's/\r$//' /tmp/cicd.env /usr/local/bin/install-libvship-linux.sh /usr/local/bin/install-fmetrics-linux.sh && \
    set -a && . /tmp/cicd.env && set +a && \
    work="$(mktemp -d /tmp/zig-toolchain.XXXXXX)" && \
    trap 'rm -rf "$work"' EXIT && \
    bash /usr/local/bin/download-verified.sh "${FMETRICS_ZIG_LINUX_X86_64_SHA256:-}" "$work/zig.tar.xz" \
        "https://ziglang.org/download/${FMETRICS_ZIG_VERSION}/zig-x86_64-linux-${FMETRICS_ZIG_VERSION}.tar.xz" && \
    mkdir -p /opt/zig && \
    tar -xJf "$work/zig.tar.xz" --strip-components=1 -C /opt/zig && \
    ln -s /opt/zig/zig /usr/local/bin/zig && \
    actual="$(zig version)" && \
    if [ "$actual" != "$FMETRICS_ZIG_VERSION" ]; then \
        echo "zig '$actual' found, but expected exactly $FMETRICS_ZIG_VERSION; refusing build." >&2; exit 1; \
    fi && \
    bash /usr/local/bin/install-libvship-linux.sh -Destination /usr/lib -Quiet && \
    bash /usr/local/bin/install-fmetrics-linux.sh -Destination /usr/lib -Quiet && \
    rm -f /tmp/cicd.env

FROM base AS runtime

ENV MPLCONFIGDIR="/home/app_user/"

# libvship imports the Vulkan loader, so the loader must be present even though
# no driver is.
RUN pacman -S --noconfirm --needed vulkan-icd-loader

COPY --from=metrics /usr/lib/libvship.so /usr/lib/libvship.so
COPY --from=metrics /usr/lib/libfmetrics.so /usr/lib/libfmetrics.so

COPY --from=build /usr/local/bin/av1an /usr/local/bin/av1an

# Create user
RUN useradd -ms /bin/bash app_user
USER app_user

VOLUME ["/videos"]
WORKDIR /videos

ENTRYPOINT [ "/usr/local/bin/av1an" ]
