#!/bin/bash
set -e

# Write environment variables to runner .env file.
# The GitHub Actions runner passes .env vars to workflow steps.
cat > /home/runner/actions-runner/.env <<EOF
TAPA_LICENSE_DIR=${TAPA_LICENSE_DIR}
TAPA_TOOLS_DIR=${TAPA_TOOLS_DIR}
TAPA_PLATFORMS_DIR=${TAPA_PLATFORMS_DIR}
EOF
chown runner:runner /home/runner/actions-runner/.env

# Configure runner if not already configured
if [ ! -f /home/runner/actions-runner/.runner ]; then
    if [ -z "$RUNNER_TOKEN" ]; then
        echo "ERROR: RUNNER_TOKEN is required for first-time setup"
        exit 1
    fi

    su -c "cd /home/runner/actions-runner && ./config.sh \
        --url https://github.com/${RUNNER_REPO:-tuna/tapa} \
        --token ${RUNNER_TOKEN} \
        --name ${RUNNER_NAME:-tapa-ci-runner} \
        --labels ${RUNNER_LABELS:-self-hosted,linux,x64} \
        --work _work \
        --unattended \
        --replace" runner
fi

# Start supervisord (manages dockerd + runner)
exec /usr/bin/supervisord -c /etc/supervisor/conf.d/supervisord.conf
