"""
eGPU Manager LLM Gateway Client Library.

Provides a simple interface for Python applications to use the
LLM Gateway running on the eGPU Manager (localhost:7842).

Usage:
    from egpu_llm_client import EgpuLlmClient

    client = EgpuLlmClient(app_id="audit_designer")

    # Check if gateway is available
    if client.is_available():
        response = client.chat("Analysiere diesen Beleg", model="qwen3:14b")
        print(response["choices"][0]["message"]["content"])

    # Streaming
    for chunk in client.chat_stream("Erkläre mir das", model="qwen3:14b"):
        print(chunk, end="", flush=True)

    # Embeddings via Gateway (GPU-aware routing)
    embeddings = client.embed("Testtext für Embedding")

    # Staging für Bulk-Operationen (VRAM reservieren)
    lease = client.staging_start("embeddings", vram_mb=4000, duration_s=7200)
    # ... Bulk-Arbeit ...
    client.staging_end(lease["lease_id"])
"""

from __future__ import annotations

import json
import logging
import os
import time
import threading
from typing import Any, Generator, Optional

import requests

logger = logging.getLogger(__name__)


class EgpuLlmClient:
    """Client for the eGPU Manager LLM Gateway."""

    def __init__(
        self,
        app_id: str,
        gateway_url: str | None = None,
        timeout: int = 120,
    ):
        self.app_id = app_id
        self.gateway_url = (
            gateway_url
            or os.environ.get("EGPU_GATEWAY_URL")
            or "http://localhost:7842"
        )
        self.timeout = timeout
        self._session = requests.Session()
        self._session.headers.update(
            {
                "X-App-Id": self.app_id,
                "Content-Type": "application/json",
            }
        )

    def is_available(self) -> bool:
        """Check if the LLM Gateway is reachable and healthy."""
        try:
            resp = self._session.get(
                f"{self.gateway_url}/api/llm/health",
                timeout=5,
            )
            return resp.status_code == 200
        except requests.ConnectionError:
            return False

    def chat(
        self,
        prompt: str,
        *,
        model: str = "qwen3:14b",
        system: str | None = None,
        messages: list[dict[str, str]] | None = None,
        temperature: float | None = None,
        max_tokens: int | None = None,
        provider: str | None = None,
        workload_type: str | None = None,
    ) -> dict[str, Any]:
        """Send a chat completion request.

        Args:
            prompt: User message (ignored if messages is provided)
            model: Model name
            system: Optional system prompt
            messages: Full message list (overrides prompt/system)
            temperature: Sampling temperature
            max_tokens: Maximum tokens to generate
            provider: Force a specific provider
            workload_type: Workload hint for GPU routing (e.g. "llm", "ocr")

        Returns:
            OpenAI-compatible response dict

        Raises:
            EgpuGatewayError: On gateway errors (rate limit, budget, etc.)
            requests.ConnectionError: If gateway is unreachable
        """
        if messages is None:
            messages = []
            if system:
                messages.append({"role": "system", "content": system})
            messages.append({"role": "user", "content": prompt})

        body: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "stream": False,
        }
        if temperature is not None:
            body["temperature"] = temperature
        if max_tokens is not None:
            body["max_tokens"] = max_tokens
        if provider is not None:
            body["provider"] = provider
        if workload_type is not None:
            body["workload_type"] = workload_type

        resp = self._session.post(
            f"{self.gateway_url}/api/llm/chat/completions",
            json=body,
            timeout=self.timeout,
        )

        if resp.status_code != 200:
            data = resp.json()
            raise EgpuGatewayError(
                status=resp.status_code,
                error_type=data.get("error", {}).get("type", "unknown"),
                message=data.get("error", {}).get("message", resp.text),
            )

        return resp.json()

    def chat_stream(
        self,
        prompt: str,
        *,
        model: str = "qwen3:14b",
        system: str | None = None,
        messages: list[dict[str, str]] | None = None,
        temperature: float | None = None,
        max_tokens: int | None = None,
        provider: str | None = None,
        workload_type: str | None = None,
    ) -> Generator[str, None, None]:
        """Send a streaming chat completion request.

        Yields content strings as they arrive.
        """
        if messages is None:
            messages = []
            if system:
                messages.append({"role": "system", "content": system})
            messages.append({"role": "user", "content": prompt})

        body: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "stream": True,
        }
        if temperature is not None:
            body["temperature"] = temperature
        if max_tokens is not None:
            body["max_tokens"] = max_tokens
        if provider is not None:
            body["provider"] = provider
        if workload_type is not None:
            body["workload_type"] = workload_type

        resp = self._session.post(
            f"{self.gateway_url}/api/llm/chat/completions",
            json=body,
            timeout=self.timeout,
            stream=True,
        )

        if resp.status_code != 200:
            data = resp.json()
            raise EgpuGatewayError(
                status=resp.status_code,
                error_type=data.get("error", {}).get("type", "unknown"),
                message=data.get("error", {}).get("message", resp.text),
            )

        for line in resp.iter_lines(decode_unicode=True):
            if not line or line.strip() == "data: [DONE]":
                continue
            if line.startswith("data: "):
                try:
                    chunk = json.loads(line[6:])
                    content = (
                        chunk.get("choices", [{}])[0]
                        .get("delta", {})
                        .get("content", "")
                    )
                    if content:
                        yield content
                except (json.JSONDecodeError, IndexError, KeyError):
                    continue

    def embed(
        self,
        input: str | list[str],
        *,
        model: str = "auto",
    ) -> dict[str, Any] | None:
        """Generate embeddings via the Gateway's embedding proxy.

        The Gateway routes to the optimal Ollama instance with GPU awareness.
        Model "auto" resolves to the app's configured embedding model
        (e.g. nomic-embed-text for audit_designer).

        ACHTUNG: nomic-embed-text (768d) und paraphrase-multilingual-mpnet-base-v2
        (768d) haben identische Dimensionen aber verschiedene Vektorräume.
        Bei Provider-Wechsel muss komplett neu-embedded werden!

        Args:
            input: Text or list of texts to embed
            model: Model name ("auto" = app-configured default)

        Returns:
            OpenAI-compatible embedding response dict, or None on connection error
        """
        body: dict[str, Any] = {
            "model": model,
            "input": input,
        }

        try:
            resp = self._session.post(
                f"{self.gateway_url}/api/llm/embeddings",
                json=body,
                timeout=self.timeout,
            )
        except requests.ConnectionError:
            logger.warning("eGPU Gateway nicht erreichbar für Embedding-Request")
            return None

        if resp.status_code != 200:
            try:
                data = resp.json()
                raise EgpuGatewayError(
                    status=resp.status_code,
                    error_type=data.get("error", {}).get("type", "unknown"),
                    message=data.get("error", {}).get("message", resp.text),
                )
            except (ValueError, KeyError):
                raise EgpuGatewayError(
                    status=resp.status_code,
                    error_type="unknown",
                    message=resp.text,
                )

        return resp.json()

    def staging_start(
        self,
        workload_type: str,
        *,
        vram_mb: int = 4000,
        duration_s: int = 7200,
        description: str = "",
    ) -> dict[str, Any] | None:
        """Reserve VRAM via staging endpoint for bulk operations.

        Use this before long-running jobs (e.g. re-embedding 50k documents)
        to ensure GPU capacity is reserved and other workloads are preempted.

        Args:
            workload_type: Type of workload ("embeddings", "llm", "ocr")
            vram_mb: Required VRAM in MB
            duration_s: Expected duration in seconds
            description: Human-readable job description

        Returns:
            Staging response with lease_id, ollama_host, model — or None on error
        """
        body: dict[str, Any] = {
            "workload_type": workload_type,
            "vram_mb": vram_mb,
            "duration_seconds": duration_s,
            "description": description,
        }

        try:
            resp = self._session.post(
                f"{self.gateway_url}/api/llm/staging",
                json=body,
                timeout=10,
            )
        except requests.ConnectionError:
            logger.warning("eGPU Gateway nicht erreichbar für Staging-Request")
            return None

        if resp.status_code != 200:
            logger.warning(
                "Staging-Request fehlgeschlagen: %s %s",
                resp.status_code, resp.text[:200],
            )
            return None

        return resp.json()

    def staging_end(self, lease_id: str) -> bool:
        """Release a staging reservation.

        Args:
            lease_id: The lease_id from staging_start()

        Returns:
            True if successfully released
        """
        try:
            resp = self._session.post(
                f"{self.gateway_url}/api/gpu/release",
                json={"lease_id": lease_id, "success": True},
                timeout=5,
            )
            return resp.status_code == 200
        except requests.ConnectionError:
            logger.warning("eGPU Gateway nicht erreichbar für Release")
            return False

    def heartbeat(self, lease_id: str) -> bool:
        """Send heartbeat for a long-running staging lease.

        Prevents lease timeout for bulk jobs >2h. Call periodically
        (e.g. every 10 minutes) during long operations.

        Args:
            lease_id: The lease_id from staging_start()

        Returns:
            True if heartbeat acknowledged
        """
        try:
            resp = self._session.post(
                f"{self.gateway_url}/api/gpu/heartbeat",
                json={"lease_id": lease_id},
                timeout=5,
            )
            return resp.status_code == 200
        except requests.ConnectionError:
            return False

    def get_providers(self) -> list[dict[str, Any]]:
        """Get list of available LLM providers."""
        resp = self._session.get(
            f"{self.gateway_url}/api/llm/providers",
            timeout=5,
        )
        resp.raise_for_status()
        return resp.json().get("providers", [])

    def get_usage(self) -> dict[str, Any]:
        """Get usage statistics for this app."""
        resp = self._session.get(
            f"{self.gateway_url}/api/llm/usage/{self.app_id}",
            timeout=5,
        )
        resp.raise_for_status()
        return resp.json()

    def close(self):
        """Close the HTTP session."""
        self._session.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


class EgpuGatewayError(Exception):
    """Error from the eGPU LLM Gateway."""

    def __init__(self, status: int, error_type: str, message: str):
        self.status = status
        self.error_type = error_type
        self.message = message
        super().__init__(f"[{status}] {error_type}: {message}")

    @property
    def is_rate_limited(self) -> bool:
        return self.error_type == "rate_limit_error"

    @property
    def is_budget_exceeded(self) -> bool:
        return self.error_type == "budget_exceeded"

    @property
    def is_provider_error(self) -> bool:
        return self.error_type == "provider_error"
