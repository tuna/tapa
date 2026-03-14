"""Thread-safe SSH connection pool using paramiko."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import contextlib
import logging
import threading

import paramiko

from tapa.remote.config import RemoteConfig

_logger = logging.getLogger().getChild(__name__)


class SSHConnectionPool:
    """Thread-safe SSH connection pool.

    Uses threading.local() to maintain per-thread paramiko.SSHClient instances,
    since paramiko connections are not thread-safe and run_hls() uses
    ThreadPoolExecutor.
    """

    def __init__(self) -> None:
        self._local = threading.local()

    def get_connection(self, config: RemoteConfig) -> paramiko.SSHClient:
        """Get or create a per-thread SSH connection."""
        key = (config.host, config.port, config.user)

        if not hasattr(self._local, "connections"):
            self._local.connections = {}

        conn = self._local.connections.get(key)
        if conn is not None:
            transport = conn.get_transport()
            if transport and transport.is_active():
                return conn

        client = paramiko.SSHClient()
        client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        _logger.warning(
            "Auto-adding host key for %s (paramiko.AutoAddPolicy)", config.host
        )

        connect_kwargs: dict = {
            "hostname": config.host,
            "port": config.port,
            "username": config.user,
        }
        if config.key_file:
            connect_kwargs["key_filename"] = config.key_file
        # Otherwise, paramiko tries SSH agent, then default keys

        _logger.info(
            "Opening SSH connection to %s@%s:%d",
            config.user,
            config.host,
            config.port,
        )
        client.connect(**connect_kwargs)

        self._local.connections[key] = client
        return client

    def close_all(self) -> None:
        """Close all connections for the current thread."""
        if hasattr(self._local, "connections"):
            for conn in self._local.connections.values():
                with contextlib.suppress(Exception):
                    conn.close()
            self._local.connections.clear()


_pool = SSHConnectionPool()


def get_connection(config: RemoteConfig) -> paramiko.SSHClient:
    """Get an SSH connection from the global pool."""
    return _pool.get_connection(config)
