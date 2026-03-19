"""Tests für EgpuLlmClient — Gateway-Methoden.

Testet alle neuen Methoden (embed, staging_start/end, heartbeat)
sowie workload_type Parameter. Verwendet Mock-HTTP statt echtes Gateway.
"""

import json
from unittest.mock import MagicMock, patch

import pytest

from egpu_llm_client import EgpuGatewayError, EgpuLlmClient


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def client():
    """EgpuLlmClient mit Test-URL."""
    return EgpuLlmClient(app_id="test_app", gateway_url="http://test:7842")


def _mock_response(status_code=200, json_data=None, text=""):
    """Erstellt Mock-Response."""
    resp = MagicMock()
    resp.status_code = status_code
    resp.text = text or json.dumps(json_data or {})
    resp.json.return_value = json_data or {}
    resp.iter_lines.return_value = []
    return resp


# ---------------------------------------------------------------------------
# embed() Tests
# ---------------------------------------------------------------------------


class TestEmbed:
    def test_embed_single_text(self, client):
        """embed() sendet korrekten Request und gibt Response zurück."""
        mock_resp = _mock_response(
            json_data={
                "data": [{"embedding": [0.1] * 768, "index": 0}],
                "model": "nomic-embed-text",
            }
        )
        client._session.post = MagicMock(return_value=mock_resp)

        result = client.embed("Testtext")

        assert result is not None
        assert len(result["data"]) == 1
        assert len(result["data"][0]["embedding"]) == 768

        # Verify request
        call_args = client._session.post.call_args
        assert "/api/llm/embeddings" in call_args[0][0]
        body = call_args[1]["json"]
        assert body["model"] == "auto"
        assert body["input"] == "Testtext"

    def test_embed_batch_texts(self, client):
        """embed() mit Liste sendet Batch-Request."""
        texts = ["Text 1", "Text 2", "Text 3"]
        mock_resp = _mock_response(
            json_data={
                "data": [
                    {"embedding": [0.1] * 768, "index": i} for i in range(3)
                ],
            }
        )
        client._session.post = MagicMock(return_value=mock_resp)

        result = client.embed(texts)

        body = client._session.post.call_args[1]["json"]
        assert body["input"] == texts
        assert len(result["data"]) == 3

    def test_embed_custom_model(self, client):
        """embed() mit explizitem Model."""
        mock_resp = _mock_response(json_data={"data": []})
        client._session.post = MagicMock(return_value=mock_resp)

        client.embed("Test", model="nomic-embed-text")

        body = client._session.post.call_args[1]["json"]
        assert body["model"] == "nomic-embed-text"

    def test_embed_connection_error_returns_none(self, client):
        """embed() gibt None zurück bei ConnectionError (kein Crash)."""
        import requests

        client._session.post = MagicMock(side_effect=requests.ConnectionError())

        result = client.embed("Test")
        assert result is None

    def test_embed_gateway_error_raises(self, client):
        """embed() wirft EgpuGatewayError bei HTTP-Fehler."""
        mock_resp = _mock_response(
            status_code=429,
            json_data={"error": {"type": "rate_limit_error", "message": "Too many"}},
        )
        client._session.post = MagicMock(return_value=mock_resp)

        with pytest.raises(EgpuGatewayError) as exc_info:
            client.embed("Test")
        assert exc_info.value.is_rate_limited

    def test_embed_x_app_id_header(self, client):
        """embed() sendet X-App-Id Header."""
        assert client._session.headers["X-App-Id"] == "test_app"


# ---------------------------------------------------------------------------
# staging_start() / staging_end() Tests
# ---------------------------------------------------------------------------


class TestStaging:
    def test_staging_start_success(self, client):
        """staging_start() reserviert VRAM und gibt Lease-Info zurück."""
        mock_resp = _mock_response(
            json_data={
                "lease_id": "lease-abc123",
                "ollama_host": "http://ollama-egpu:11434",
                "model": "nomic-embed-text",
            }
        )
        client._session.post = MagicMock(return_value=mock_resp)

        result = client.staging_start(
            "embeddings", vram_mb=4000, duration_s=7200, description="Re-tag 50k"
        )

        assert result["lease_id"] == "lease-abc123"
        body = client._session.post.call_args[1]["json"]
        assert body["workload_type"] == "embeddings"
        assert body["vram_mb"] == 4000
        assert body["duration_seconds"] == 7200
        assert body["description"] == "Re-tag 50k"

    def test_staging_start_gateway_down_returns_none(self, client):
        """staging_start() gibt None zurück wenn Gateway nicht erreichbar."""
        import requests

        client._session.post = MagicMock(side_effect=requests.ConnectionError())

        result = client.staging_start("embeddings")
        assert result is None

    def test_staging_start_error_returns_none(self, client):
        """staging_start() gibt None bei HTTP-Fehler zurück (kein Exception)."""
        mock_resp = _mock_response(status_code=503, text="Service Unavailable")
        client._session.post = MagicMock(return_value=mock_resp)

        result = client.staging_start("embeddings")
        assert result is None

    def test_staging_end_success(self, client):
        """staging_end() gibt Lease frei."""
        mock_resp = _mock_response(json_data={"released": True})
        client._session.post = MagicMock(return_value=mock_resp)

        result = client.staging_end("lease-abc123")

        assert result is True
        body = client._session.post.call_args[1]["json"]
        assert body["lease_id"] == "lease-abc123"
        assert body["success"] is True

    def test_staging_end_gateway_down_returns_false(self, client):
        """staging_end() gibt False zurück wenn Gateway nicht erreichbar."""
        import requests

        client._session.post = MagicMock(side_effect=requests.ConnectionError())

        result = client.staging_end("lease-abc123")
        assert result is False


# ---------------------------------------------------------------------------
# heartbeat() Tests
# ---------------------------------------------------------------------------


class TestHeartbeat:
    def test_heartbeat_success(self, client):
        """heartbeat() sendet lease_id."""
        mock_resp = _mock_response(json_data={"ok": True})
        client._session.post = MagicMock(return_value=mock_resp)

        result = client.heartbeat("lease-abc123")

        assert result is True
        body = client._session.post.call_args[1]["json"]
        assert body["lease_id"] == "lease-abc123"

    def test_heartbeat_connection_error(self, client):
        """heartbeat() gibt False zurück bei Connection-Fehler."""
        import requests

        client._session.post = MagicMock(side_effect=requests.ConnectionError())
        assert client.heartbeat("lease-abc123") is False


# ---------------------------------------------------------------------------
# workload_type Parameter Tests
# ---------------------------------------------------------------------------


class TestWorkloadType:
    def test_chat_with_workload_type(self, client):
        """chat() sendet workload_type im Body."""
        mock_resp = _mock_response(
            json_data={
                "choices": [{"message": {"content": "OK"}}],
                "model": "qwen3:14b",
            }
        )
        client._session.post = MagicMock(return_value=mock_resp)

        client.chat("Hallo", workload_type="llm")

        body = client._session.post.call_args[1]["json"]
        assert body["workload_type"] == "llm"

    def test_chat_without_workload_type(self, client):
        """chat() ohne workload_type sendet keinen workload_type Key."""
        mock_resp = _mock_response(
            json_data={
                "choices": [{"message": {"content": "OK"}}],
            }
        )
        client._session.post = MagicMock(return_value=mock_resp)

        client.chat("Hallo")

        body = client._session.post.call_args[1]["json"]
        assert "workload_type" not in body

    def test_chat_stream_with_workload_type(self, client):
        """chat_stream() sendet workload_type."""
        mock_resp = _mock_response(
            json_data={
                "choices": [{"message": {"content": "OK"}}],
            }
        )
        mock_resp.iter_lines.return_value = ["data: [DONE]"]
        client._session.post = MagicMock(return_value=mock_resp)

        # Consume generator
        list(client.chat_stream("Hallo", workload_type="ocr"))

        body = client._session.post.call_args[1]["json"]
        assert body["workload_type"] == "ocr"


# ---------------------------------------------------------------------------
# Full Workflow Integration Test
# ---------------------------------------------------------------------------


class TestStagingWorkflow:
    def test_full_staging_lifecycle(self, client):
        """Kompletter Staging-Workflow: start → heartbeat → end."""
        responses = [
            # staging_start
            _mock_response(json_data={"lease_id": "lease-xyz", "ollama_host": "http://ollama:11434"}),
            # heartbeat
            _mock_response(json_data={"ok": True}),
            # embed
            _mock_response(json_data={"data": [{"embedding": [0.1] * 768}]}),
            # staging_end
            _mock_response(json_data={"released": True}),
        ]
        client._session.post = MagicMock(side_effect=responses)

        # 1. Start
        staging = client.staging_start("embeddings", vram_mb=4000)
        assert staging["lease_id"] == "lease-xyz"

        # 2. Heartbeat
        assert client.heartbeat(staging["lease_id"]) is True

        # 3. Embed
        result = client.embed("Test-Dokument")
        assert result is not None

        # 4. End
        assert client.staging_end(staging["lease_id"]) is True

        assert client._session.post.call_count == 4
