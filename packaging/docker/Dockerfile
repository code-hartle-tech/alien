# Alien in a container — CLI only, and read the caveat before using it.
#
# A container can reach /proc/acpi/call only with --privileged and the host's
# /proc, which is most of the isolation gone. This image exists because it is
# genuinely useful for CI and for a one-shot `alien fan max` on a headless box,
# NOT because it is a sensible way to run a desktop tool. If you want the GUI,
# install a native package.
#
#   docker run --rm --privileged -v /proc/acpi/call:/proc/acpi/call \
#     ghcr.io/hartle-tech/alien status
#
# The acpi_call module must already be loaded on the HOST. A container cannot
# load kernel modules, and if it could, it would be loading them into the
# host's kernel — there is no container-local kernel to isolate that into.

FROM rust:1.83-slim-bookworm AS build
WORKDIR /src
COPY code/ ./code/
# Only the CLI: the GUI would drag a GL and Wayland stack into an image that
# has no display, and the daemon has no reason to exist here — with
# --privileged the CLI reaches the firmware directly.
RUN cd code && cargo build --release -p alien-cli && \
    strip target/release/alien

# Distroless-style final stage. The binary links only libc, so there is nothing
# else to carry.
FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="Alien" \
      org.opencontainers.image.description="Acer Predator/Nitro hardware control" \
      org.opencontainers.image.licenses="GPL-2.0-or-later" \
      org.opencontainers.image.url="https://alien.hartle.tech"

COPY --from=build /src/code/target/release/alien /usr/bin/alien

# Root on purpose: writing /proc/acpi/call requires it, and this image is only
# ever run --privileged. Dropping to a user here would be security theatre —
# it would break the one thing the image does while changing nothing about the
# host exposure that --privileged already grants.
USER root
ENTRYPOINT ["/usr/bin/alien"]
CMD ["status"]
