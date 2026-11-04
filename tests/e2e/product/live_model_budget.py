"""One durable USD budget shared by every real-provider acceptance request."""

from __future__ import annotations

import json
import os
import threading
import uuid
from decimal import Decimal
from pathlib import Path
from typing import Any


class BudgetError(ValueError):
    """An admission or accounting failure containing no provider request data."""


class ModelBudget:
    def __init__(
        self,
        report: Path,
        *,
        limit: Decimal,
        input_price: Decimal,
        output_price: Decimal,
        max_requests: int | None = None,
    ):
        if not all(value.is_finite() and value > 0 for value in (limit, input_price, output_price)):
            raise BudgetError("Budget and USD token prices must be finite and positive")
        if max_requests is not None and (type(max_requests) is not int or max_requests <= 0):
            raise BudgetError("A positive request limit is required")
        self.max_requests = max_requests
        self.report = report
        self.limit = limit
        self.input_price = input_price
        self.output_price = output_price
        self.lock = threading.Lock()
        self.spent = Decimal(0)
        self.pending: dict[str, dict[str, Any]] = {}
        self.requests = 0
        self.settled = 0
        self.halted = False
        if report.exists():
            data = json.loads(report.read_text())
            identity = (str(limit), str(input_price), str(output_price))
            if (
                data.get("version") != 2
                or data["maxRequests"] != max_requests
                or tuple(data[key] for key in ("limitUsd", "inputPerMillion", "outputPerMillion")) != identity
            ):
                raise BudgetError("Existing budget identity differs; a new run cannot reset this ledger")
            self.spent = Decimal(data["spentUsd"])
            self.pending = data["pending"]
            self.requests = data["requests"]
            self.settled = data["settled"]
            self.halted = data["halted"]
            for reservation in self.pending.values():
                reservation["state"] = "unknown"
        with self.lock:
            self._save()

    def _cost(self, input_tokens: int, output_tokens: int) -> Decimal:
        return (Decimal(input_tokens) * self.input_price + Decimal(output_tokens) * self.output_price) / 1_000_000

    def _snapshot(self) -> dict:
        held = sum((Decimal(value["reservedUsd"]) for value in self.pending.values()), Decimal(0))
        return {
            "version": 2,
            "maxRequests": self.max_requests,
            "limitUsd": str(self.limit),
            "inputPerMillion": str(self.input_price),
            "outputPerMillion": str(self.output_price),
            "spentUsd": str(self.spent),
            "heldUsd": str(held),
            "availableUsd": str(max(Decimal(0), self.limit - self.spent - held)),
            "requests": self.requests,
            "settled": self.settled,
            "halted": self.halted,
            "pending": {key: dict(value) for key, value in self.pending.items()},
        }

    def snapshot(self) -> dict:
        with self.lock:
            return self._snapshot()

    def _save(self) -> None:
        self.report.parent.mkdir(parents=True, exist_ok=True)
        temporary = self.report.with_suffix(".writing")
        temporary.write_text(json.dumps(self._snapshot(), indent=2) + "\n")
        os.replace(temporary, self.report)

    @staticmethod
    def _bounds(request: dict) -> tuple[int, int]:
        if request.get("model") != "glm-5.3-flash":
            raise BudgetError("Only the explicitly authorized model may be called")
        maximum = request.get("max_tokens", request.get("max_completion_tokens"))
        if type(maximum) is not int or not 1 <= maximum <= 16000:
            raise BudgetError("Each request requires an output limit between 1 and 16000 tokens")
        if (
            "max_tokens" in request
            and "max_completion_tokens" in request
            and request["max_tokens"] != request["max_completion_tokens"]
        ):
            raise BudgetError("Conflicting output token limits")
        messages = request.get("messages")
        tools = request.get("tools", [])
        if not isinstance(messages, list) or not messages or not isinstance(tools, list):
            raise BudgetError("A messages array and valid tools array are required")
        content = json.dumps({"messages": messages, "tools": tools}, ensure_ascii=False).encode()
        if len(content) > 8 * 1024 * 1024:
            raise BudgetError("Model request exceeds the acceptance size limit")
        # UTF-8 bytes conservatively bound text tokenization. Multimodal parts
        # reserve the model's full context ceiling instead of guessing patches.
        media = sum(
            1
            for message in messages
            if isinstance(message, dict) and isinstance(message.get("content"), list)
            for part in message["content"]
            if isinstance(part, dict) and part.get("type") not in ("text", "input_text")
        )
        ceiling = len(content) + 128 * (len(messages) + len(tools) + 1) + media * 1_000_000
        return ceiling, maximum

    def reserve(self, request: dict) -> str:
        input_limit, output_limit = self._bounds(request)
        reserved = self._cost(input_limit, output_limit)
        with self.lock:
            available = Decimal(self._snapshot()["availableUsd"])
            if (
                self.halted
                or reserved > available
                or (self.max_requests is not None and self.requests >= self.max_requests)
            ):
                raise BudgetError("REAL_MODEL_BUDGET_EXHAUSTED")
            identifier = str(uuid.uuid4())
            self.pending[identifier] = {
                "reservedUsd": str(reserved),
                "inputCeiling": input_limit,
                "outputLimit": output_limit,
                "state": "in_flight",
            }
            self.requests += 1
            self._save()
            return identifier

    def settle(self, identifier: str, usage: dict) -> None:
        input_tokens = usage.get("prompt_tokens")
        output_tokens = usage.get("completion_tokens")
        if any(type(value) is not int or value < 0 for value in (input_tokens, output_tokens)):
            self.unknown(identifier)
            raise BudgetError("Provider usage was missing or invalid; the reservation is retained")
        with self.lock:
            reservation = self.pending.get(identifier)
            if reservation is None:
                raise BudgetError("A provider response cannot settle an unknown or settled reservation")
            cost = self._cost(input_tokens, output_tokens)
            self.spent += cost
            self.settled += 1
            del self.pending[identifier]
            exceeded = input_tokens > reservation["inputCeiling"] or output_tokens > reservation["outputLimit"]
            self.halted = self.halted or exceeded or self.spent > self.limit
            self._save()
            if exceeded:
                raise BudgetError("Provider exceeded the admitted token bounds; real-model acceptance is halted")

    def unknown(self, identifier: str) -> None:
        with self.lock:
            if identifier in self.pending:
                self.pending[identifier]["state"] = "unknown"
                self._save()

    def unsent(self, identifier: str) -> None:
        """Only a proven connection failure before submission may release a hold."""
        with self.lock:
            if identifier not in self.pending or self.pending[identifier]["state"] != "in_flight":
                raise BudgetError("Unknown provider outcomes cannot be released as unsent")
            del self.pending[identifier]
            self._save()
