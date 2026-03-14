FROM debian:bookworm

# Install base dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git iptables supervisor \
    libicu72 libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Install Docker (from official static binaries)
ARG DOCKER_VERSION=28.3.1
RUN curl -fsSL "https://download.docker.com/linux/static/stable/x86_64/docker-${DOCKER_VERSION}.tgz" \
    | tar -xz -C /usr/local/bin --strip-components=1 \
      docker/docker docker/dockerd docker/containerd \
      docker/containerd-shim-runc-v2 docker/runc

# Provide docker-init (required by --init flag) via statically linked tini.
# Must be static so it works inside containers with different libc (e.g. musl).
ARG TINI_VERSION=v0.19.0
RUN curl -fsSL "https://github.com/krallin/tini/releases/download/${TINI_VERSION}/tini-static" \
      -o /usr/local/bin/docker-init && \
    chmod +x /usr/local/bin/docker-init

# Configure Docker daemon.
# Set DOCKER_REGISTRY_MIRROR at build time to use a pull-through mirror,
# or override at runtime by bind-mounting /etc/docker/daemon.json.
ARG DOCKER_REGISTRY_MIRROR=
RUN mkdir -p /etc/docker && \
    if [ -n "${DOCKER_REGISTRY_MIRROR}" ]; then \
      printf '{"storage-driver":"overlay2","registry-mirrors":["%s"]}\n' "${DOCKER_REGISTRY_MIRROR}" \
        > /etc/docker/daemon.json; \
    else \
      echo '{"storage-driver":"overlay2"}' > /etc/docker/daemon.json; \
    fi

# Install GitHub Actions runner
ARG RUNNER_VERSION=2.332.0
RUN groupadd docker && \
    useradd -m -d /home/runner -s /bin/bash -G docker runner \
    && mkdir -p /home/runner/actions-runner \
    && cd /home/runner/actions-runner \
    && curl -fsSL "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz" \
       | tar -xz \
    && ./bin/installdependencies.sh \
    && chown -R runner:runner /home/runner/actions-runner

# Default TAPA environment variables for the runner.
# Override at runtime via -e flags to match the host's tool paths.
ENV TAPA_LICENSE_DIR=/share/software/licenses
ENV TAPA_TOOLS_DIR=/share/software/tools
ENV TAPA_PLATFORMS_DIR=/share/software/xilinx/platforms

# Supervisord config to run dockerd + runner
RUN mkdir -p /var/log/supervisor
COPY supervisord.conf /etc/supervisor/conf.d/supervisord.conf
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
