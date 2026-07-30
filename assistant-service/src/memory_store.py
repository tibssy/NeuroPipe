import os
import time
import uuid
from collections.abc import Callable

from qdrant_client import QdrantClient
from qdrant_client.http.models import Distance, PointStruct, VectorParams


class MemoryStore:
    def __init__(
        self,
        qdrant_path: str,
        collection: str,
        embedding_model: str,
        embed_texts: Callable[[str, list[str]], list[list[float]]],
    ):
        self.qdrant_path = os.path.expanduser(qdrant_path)
        self.collection_name = collection
        self.embedding_model = embedding_model
        self.embed_texts = embed_texts

        self._client = None
        self._vector_size = None
        self._disabled_reason = None
        self._warned = False

    def close(self):
        if self._client is None:
            return
        try:
            self._client.close()
        except Exception:
            pass
        finally:
            self._client = None

    def _disable(self, reason: str):
        self._disabled_reason = reason

    def _warn_once(self):
        if not self._warned and self._disabled_reason:
            print(f"[memory] Disabled: {self._disabled_reason}")
            self._warned = True

    def _ensure_client(self):
        if self._disabled_reason:
            self._warn_once()
            return None

        if self._client is not None:
            return self._client

        try:
            os.makedirs(self.qdrant_path, exist_ok=True)
            self._client = QdrantClient(path=self.qdrant_path)
            return self._client
        except Exception as e:
            self._disable(str(e))
            self._warn_once()
            return None

    def _ensure_collection(self, vector_size: int):
        client = self._ensure_client()
        if client is None:
            return False

        try:
            exists = client.collection_exists(self.collection_name)
            if not exists:
                client.create_collection(
                    collection_name=self.collection_name,
                    vectors_config=VectorParams(size=vector_size, distance=Distance.COSINE),
                )
                self._vector_size = vector_size
                return True

            if self._vector_size is None:
                info = client.get_collection(self.collection_name)
                config = info.config.params.vectors
                if hasattr(config, "size"):
                    self._vector_size = int(config.size)
            return True
        except Exception as e:
            self._disable(str(e))
            self._warn_once()
            return False

    def _embed_one(self, text: str) -> list[float] | None:
        if not text:
            return None

        try:
            vectors = self.embed_texts(self.embedding_model, [text])
        except Exception as e:
            self._disable(f"Embedding failed: {e}")
            self._warn_once()
            return None

        if not vectors or not vectors[0]:
            return None
        return [float(v) for v in vectors[0]]

    def add_summary(self, summary: str, metadata: dict):
        vector = self._embed_one(summary)
        if not vector:
            return False

        if not self._ensure_collection(len(vector)):
            return False

        client = self._ensure_client()
        if client is None:
            return False

        safe_meta = {k: str(v) for k, v in metadata.items()}
        safe_meta["saved_at"] = str(int(time.time()))

        try:
            point = PointStruct(
                id=str(uuid.uuid4()),
                vector=vector,
                payload={"document": summary, "metadata": safe_meta},
            )
            client.upsert(collection_name=self.collection_name, points=[point], wait=True)
            return True
        except Exception as e:
            self._disable(str(e))
            self._warn_once()
            return False

    def search(self, query: str, top_k: int) -> list[dict]:
        if top_k < 1:
            return []

        vector = self._embed_one(query)
        if not vector:
            return []

        if not self._ensure_collection(len(vector)):
            return []

        client = self._ensure_client()
        if client is None:
            return []

        try:
            count_result = client.count(collection_name=self.collection_name, exact=False)
            if int(count_result.count) <= 0:
                return []
        except Exception:
            return []

        try:
            if hasattr(client, "query_points"):
                response = client.query_points(
                    collection_name=self.collection_name,
                    query=vector,
                    limit=int(top_k),
                    with_payload=True,
                )
                matches = getattr(response, "points", response)
            else:
                matches = client.search(
                    collection_name=self.collection_name,
                    query_vector=vector,
                    limit=int(top_k),
                    with_payload=True,
                )
        except Exception as e:
            self._disable(str(e))
            self._warn_once()
            return []

        out = []
        for hit in matches:
            payload = hit.payload or {}
            out.append(
                {
                    "document": (payload.get("document") or "").strip(),
                    "metadata": payload.get("metadata") or {},
                }
            )
        return out

    def reset(self) -> dict:
        client = self._ensure_client()
        if client is None:
            return {"status": "disabled", "deleted": 0}

        try:
            if not client.collection_exists(self.collection_name):
                return {"status": "ok", "deleted": 0}

            count_result = client.count(collection_name=self.collection_name, exact=True)
            deleted = int(count_result.count)
            client.delete_collection(collection_name=self.collection_name)
            self._vector_size = None
            return {"status": "ok", "deleted": deleted}
        except Exception as e:
            self._disable(str(e))
            self._warn_once()
            return {"status": "disabled", "deleted": 0}
