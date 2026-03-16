"""
eGPU Manager client for external applications.

Supports discovery, recommendation, GPU lease acquire/release and handles
both local CUDA targets and remote service targets.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any
import os

import requests


DEFAULT_TIMEOUT = 5


@dataclass(slots=True)
class GpuLease:
    lease_id: str
    gpu_device: str
    warning_level: str
    target_kind: str
    assignment_source: str
    gpu_uuid: str | None = None
    nvidia_index: int | None = None
    nvidia_visible_devices: str | None = None
    remote_gpu_name: str | None = None
    remote_host: str | None = None
    remote_ollama_url: str | None = None
    remote_agent_url: str | None = None
    expires_at: str | None = None

    @property
    def is_remote(self) -> bool:
        return self.target_kind == "remote"

    @property
    def is_local(self) -> bool:
        return not self.is_remote


class EgpuManagerClient:
    """HTTP client for the eGPU Manager local API."""

    def __init__(
        self,
        base_url: str | None = None,
        *,
        api_token: str | None = None,
        timeout: int = DEFAULT_TIMEOUT,
    ) -> None:
        self.base_url = (
            base_url
            or os.environ.get("EGPU_MANAGER_URL")
            or "http://127.0.0.1:7842"
        ).rstrip("/")
        self.timeout = timeout
        self._session = requests.Session()
        token = api_token or os.environ.get("EGPU_MANAGER_TOKEN")
        if token:
            self._session.headers["Authorization"] = f"Bearer {token}"

    def discover(self) -> dict[str, Any]:
        return self._get("/api/v1/discover")

    def recommend_gpu(
        self,
        *,
        pipeline: str | None = None,
        workload_type: str | None = None,
        vram_mb: int | None = None,
    ) -> dict[str, Any]:
        params: dict[str, Any] = {}
        if pipeline:
            params["pipeline"] = pipeline
        if workload_type:
            params["workload_type"] = workload_type
        if vram_mb is not None:
            params["vram_mb"] = vram_mb
        return self._get("/api/gpu/recommend", params=params)

    def acquire_gpu(
        self,
        *,
        pipeline: str,
        workload_type: str,
        vram_mb: int,
        duration_seconds: int = 300,
    ) -> GpuLease | None:
        try:
            data = self._post(
                "/api/gpu/acquire",
                {
                    "pipeline": pipeline,
                    "workload_type": workload_type,
                    "vram_mb": vram_mb,
                    "duration_seconds": duration_seconds,
                },
            )
        except requests.RequestException:
            return None

        if not data.get("granted"):
            return None

        return GpuLease(
            lease_id=data["lease_id"],
            gpu_device=data["gpu_device"],
            warning_level=data.get("warning_level", "unknown"),
            target_kind=data.get("target_kind", "unknown"),
            assignment_source=data.get("assignment_source", "unknown"),
            gpu_uuid=data.get("gpu_uuid"),
            nvidia_index=data.get("nvidia_index"),
            nvidia_visible_devices=data.get("nvidia_visible_devices"),
            remote_gpu_name=data.get("remote_gpu_name"),
            remote_host=data.get("remote_host"),
            remote_ollama_url=data.get("remote_ollama_url"),
            remote_agent_url=data.get("remote_agent_url"),
            expires_at=data.get("expires_at"),
        )

    def release_gpu(
        self,
        lease: GpuLease | str,
        *,
        actual_vram_mb: int | None = None,
        actual_duration_seconds: int | None = None,
        success: bool | None = None,
    ) -> dict[str, Any]:
        lease_id = lease.lease_id if isinstance(lease, GpuLease) else lease
        body: dict[str, Any] = {"lease_id": lease_id}
        if actual_vram_mb is not None:
            body["actual_vram_mb"] = actual_vram_mb
        if actual_duration_seconds is not None:
            body["actual_duration_seconds"] = actual_duration_seconds
        if success is not None:
            body["success"] = success
        return self._post("/api/gpu/release", body)

    def close(self) -> None:
        self._session.close()

    def __enter__(self) -> "EgpuManagerClient":
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def _get(self, path: str, *, params: dict[str, Any] | None = None) -> dict[str, Any]:
        response = self._session.get(
            f"{self.base_url}{path}",
            params=params,
            timeout=self.timeout,
        )
        response.raise_for_status()
        return response.json()

    def _post(self, path: str, body: dict[str, Any]) -> dict[str, Any]:
        response = self._session.post(
            f"{self.base_url}{path}",
            json=body,
            timeout=self.timeout,
        )
        response.raise_for_status()
        return response.json()
