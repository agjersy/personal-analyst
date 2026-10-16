"""JSON-RPC error codes (PROTOCOL.md section 1.2) and the exception that carries them."""

from __future__ import annotations

from typing import Any

PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603

RUN_NOT_FOUND = 1000
SANDBOX_UNAVAILABLE = 1001
PATH_OUTSIDE_ROOT = 1002
TIMEOUT = 1003
SCRIPT_NOT_FOUND = 1004
TOOL_MISSING = 1005

_MESSAGES = {
    PARSE_ERROR: "parse error",
    INVALID_REQUEST: "invalid request",
    METHOD_NOT_FOUND: "method not found",
    INVALID_PARAMS: "invalid params",
    INTERNAL_ERROR: "internal error",
    RUN_NOT_FOUND: "run not found",
    SANDBOX_UNAVAILABLE: "sandbox requested but unavailable",
    PATH_OUTSIDE_ROOT: "path outside the repo root",
    TIMEOUT: "timeout",
    SCRIPT_NOT_FOUND: "script not found or not a Python file",
    TOOL_MISSING: "tool missing on target",
}


class RpcError(Exception):
    """An error that maps directly onto a JSON-RPC error object."""

    def __init__(self, code: int, detail: str, message: str | None = None, **extra: Any):
        super().__init__(detail)
        self.code = code
        self.detail = detail
        self.message = message or _MESSAGES.get(code, "error")
        self.extra = extra

    def to_json(self) -> dict[str, Any]:
        data: dict[str, Any] = {"detail": self.detail}
        data.update(self.extra)
        return {"code": self.code, "message": self.message, "data": data}


def invalid_params(detail: str) -> RpcError:
    return RpcError(INVALID_PARAMS, detail)
