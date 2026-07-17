"""Safe, tool-grounded Intent Copilot orchestration."""

from __future__ import annotations

import json
from collections import OrderedDict
from uuid import uuid4

from intent_engine.llm import OpenAIResponsesClient
from intent_engine.llm import redact_for_prompt
from intent_engine.schemas import (
    CitedIntent,
    CopilotNotConfigured,
    CopilotQueryRequest,
    CopilotQueryResponse,
    ResumePayload,
    ResumeProposal,
)
from intent_engine.search_rewrite import rewrite_search_queries
from intent_engine.tools import ToolRegistry


SYSTEM_PROMPT = (
    "Answer only using tool results. If evidence is insufficient, say so clearly and set "
    "evidence_status to insufficient. Never invent files, URLs, commands, or dates. "
    "For resume requests, propose only an intent_id returned by a tool and copy its "
    "resume_payload exactly from get_resume_payload; never invent restore context."
)

QA_PROMPT = (
    " For grounded Q&A, retrieve evidence with tools before answering. Answer questions "
    "such as what was being fixed yesterday afternoon using only returned tool evidence. "
    "When available, include failed commands from insights.shell, and cite the relevant "
    "intent IDs."
)

BRIEFING_PROMPT = (
    " For briefing mode, retrieve the intent and its resume payload before answering. "
    "Write a concise 2-3 sentence briefing using only returned intent, insight, and "
    "resume evidence, cite the intent ID, and never invent or modify restore fields."
)

NARRATIVE_PROMPT = (
    " For narrative mode, use only the retrieved aggregate statistics and safe supporting "
    "tool results. Write a short human weekly narrative. Treat event counts, durations, "
    "intent counts, labels, projects, and dates in stats as authoritative. Never invent "
    "numbers, dates, files, URLs, commands, or resume context."
)


class IntentCopilot:
    def __init__(self, llm: OpenAIResponsesClient, tools: ToolRegistry) -> None:
        self.llm = llm
        self.tools = tools
        self._memory: OrderedDict[str, dict] = OrderedDict()
        self._memory_limit = 128

    async def query(self, request: CopilotQueryRequest) -> CopilotQueryResponse:
        await self.tools.begin_request()
        conversation_id = request.conversation_id or str(uuid4())
        messages: list[dict] = []
        prior = self._memory.get(conversation_id)
        if prior:
            messages.append({
                "role": "user",
                "content": json.dumps(
                    {
                        "prior_summary": redact_for_prompt(str(prior.get("summary", "")))[:240],
                        "prior_intent_ids": [str(i)[:128] for i in prior.get("intent_ids", [])][:20],
                    },
                    separators=(",", ":"),
                ),
            })
        messages.append({"role": "user", "content": redact_for_prompt(request.question[:2000])})
        citations: dict[str, CitedIntent] = {}
        resume_payload: ResumeProposal | None = None
        tool_names: list[str] = []
        last_answer: str | None = None
        resume_candidate: str | None = request.intent_id
        cap_reached = True
        briefing_mode = request.mode == "briefing" or (request.mode == "auto" and bool(request.intent_id))
        narrative_mode = request.mode == "narrative"
        briefing_target: str | None = request.intent_id if briefing_mode else None
        evidence_found = False

        # Briefings perform a deterministic evidence preflight. The model can then
        # write the briefing, but it cannot select or invent restore context.
        if briefing_mode:
            selected_id = request.intent_id
            if not selected_id:
                search_result = await self.tools.execute(
                    "search_intents",
                    self._with_request_filters(
                        "search_intents",
                        {"query": request.question, "limit": self.tools.context.max_results},
                        request,
                    ),
                )
                tool_names.append("search_intents")
                self._collect_citations(search_result, citations)
                records = search_result.get("results", []) if isinstance(search_result, dict) else []
                ids = []
                for record in records:
                    if isinstance(record, dict) and record.get("id") and record["id"] not in ids:
                        ids.append(record["id"])
                if len(ids) == 1:
                    selected_id = ids[0]
            briefing_target = selected_id
            if selected_id:
                resume_candidate = selected_id
                intent_result = await self.tools.execute("get_intent", {"intent_id": selected_id})
                tool_names.append("get_intent")
                self._collect_citations(intent_result, citations)
                messages.append({"role": "user", "content": json.dumps({"verified_tool_result": {"get_intent": intent_result}}, separators=(",", ":"))})
                payload_result = await self.tools.execute("get_resume_payload", {"intent_id": selected_id})
                tool_names.append("get_resume_payload")
                if "resume_payload" in payload_result:
                    try:
                        resume_payload = ResumeProposal(
                            intent_id=selected_id,
                            resume_payload=ResumePayload.model_validate(payload_result["resume_payload"]),
                        )
                    except Exception:
                        resume_payload = None
                messages.append({"role": "user", "content": json.dumps({"verified_tool_result": {"get_resume_payload": payload_result}}, separators=(",", ":"))})

        if narrative_mode:
            if request.date_from and request.date_to:
                stats_result = await self.tools.execute(
                    "get_intent_stats",
                    {
                        "date_from": request.date_from,
                        "date_to": request.date_to,
                    },
                )
                tool_names.append("get_intent_stats")
                stats_payload = stats_result.get("stats") if isinstance(stats_result, dict) else None
                evidence_found = (
                    isinstance(stats_payload, dict)
                    and int(stats_payload.get("intent_count", 0) or 0) > 0
                )
                messages.append({
                    "role": "user",
                    "content": json.dumps({"verified_tool_result": {"get_intent_stats": stats_result}}, separators=(",", ":")),
                })

        if not briefing_mode and request.mode in {"search", "auto"} and len(request.question.split()) > 3 and hasattr(self.llm, "respond_json"):
            queries = await rewrite_search_queries(request.question, self.llm)
            merged_results: dict[str, dict] = {}
            for query in queries:
                if len(tool_names) >= self.tools.context.max_tool_calls:
                    break
                result = await self.tools.execute(
                    "search_intents",
                    self._with_request_filters(
                        "search_intents", {"query": query, "limit": self.tools.context.max_results}, request
                    ),
                )
                tool_names.append("search_intents")
                self._collect_citations(result, citations)
                for record in result.get("results", []) if isinstance(result, dict) else []:
                    if isinstance(record, dict) and record.get("id"):
                        merged_results.setdefault(record["id"], {key: record.get(key) for key in ("id", "label", "summary", "date", "highlight_snippet")})
            if merged_results:
                messages.append({"role": "user", "content": json.dumps({"verified_tool_result": {"search_intents": {"results": list(merged_results.values())}}}, separators=(",", ":"))})

        for _ in range(self.tools.context.max_tool_calls):
            response = await self.llm.respond_with_tools(
                system=self._system_prompt(request),
                messages=messages,
                tools=self.tools.openai_tool_schemas(),
            )
            calls = response.get("tool_calls") or []
            output_text = response.get("output_text")
            if output_text:
                last_answer = str(output_text)[:4000]
            if not calls:
                cap_reached = False
                break

            # Responses API continuation is: prior response output items, then
            # function_call_output records bound to those call IDs. Do not send
            # Chat Completions' assistant.tool_calls / role=tool message shapes.
            messages.extend(response.get("response_items") or [])
            for call in calls:
                name = call.get("name", "")
                arguments = call.get("arguments", {})
                call_id = call.get("call_id", "")
                tool_names.append(name)
                if name == "get_resume_payload" and isinstance(arguments, dict):
                    requested_id = arguments.get("intent_id")
                    if not briefing_mode or (briefing_target and requested_id == briefing_target):
                        resume_candidate = requested_id or resume_candidate
                result = await self.tools.execute(name, self._with_request_filters(name, arguments, request))
                self._collect_citations(result, citations)
                if name == "get_resume_payload" and "resume_payload" in result and resume_candidate and (
                    not briefing_mode or (
                        briefing_target
                        and isinstance(arguments, dict)
                        and arguments.get("intent_id") == briefing_target
                    )
                ):
                    try:
                        payload = ResumePayload.model_validate(result["resume_payload"])
                        resume_payload = ResumeProposal(intent_id=resume_candidate, resume_payload=payload)
                    except Exception:
                        resume_payload = None
                messages.append({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": json.dumps(result, separators=(",", ":")),
                })

        sufficient = (bool(citations) or evidence_found) and (not briefing_mode or resume_payload is not None)
        if briefing_mode and not sufficient:
            answer = "I found no single stored intent with a verified resume payload for this briefing."
        elif narrative_mode and not sufficient:
            answer = "I found no stored statistics for this narrative."
        else:
            answer = last_answer or (
            "Tool-call limit reached before a complete answer could be produced."
            if cap_reached
            else ("I found no stored intent evidence for this question." if not sufficient else "Evidence was found in stored intents.")
            )
        summary = answer.replace("\n", " ")[:240]
        self._memory[conversation_id] = {"conversation_id": conversation_id, "summary": summary, "intent_ids": list(citations)[:20]}
        self._memory.move_to_end(conversation_id)
        while len(self._memory) > self._memory_limit:
            self._memory.popitem(last=False)
        return CopilotQueryResponse(
            answer=answer,
            citations=list(citations.values()),
            confidence=0.8 if sufficient else 0.0,
            evidence_status="sufficient" if sufficient else "insufficient",
            resume_proposal=(
                ResumeProposal(
                    intent_id=resume_payload.intent_id,
                    resume_payload=resume_payload.resume_payload,
                    briefing=answer,
                )
                if briefing_mode and resume_payload is not None and sufficient
                else (None if briefing_mode else resume_payload)
            ),
            tool_calls_made=tool_names,
            conversation_id=conversation_id,
            cached_summary=summary,
        )

    @staticmethod
    def _system_prompt(request: CopilotQueryRequest) -> str:
        if request.mode in {"qa", "auto"}:
            prompt = SYSTEM_PROMPT + QA_PROMPT
            if request.mode == "auto" and request.intent_id:
                prompt += BRIEFING_PROMPT
            return prompt
        if request.mode == "briefing":
            return SYSTEM_PROMPT + BRIEFING_PROMPT
        if request.mode == "narrative":
            return SYSTEM_PROMPT + NARRATIVE_PROMPT
        return SYSTEM_PROMPT

    @staticmethod
    def _with_request_filters(name: str, arguments: object, request: CopilotQueryRequest) -> dict:
        """Apply request-level date bounds to search/stat calls without trusting model bounds."""
        result = dict(arguments) if isinstance(arguments, dict) else {}
        if name not in {"search_intents", "get_intent_stats"}:
            return result
        if request.date_from is not None:
            result["date_from"] = request.date_from
        if request.date_to is not None:
            result["date_to"] = request.date_to
        return result

    @staticmethod
    def _collect_citations(result: dict, citations: dict[str, CitedIntent]) -> None:
        records = result.get("results", []) if isinstance(result, dict) else []
        if isinstance(result.get("intent"), dict):
            records = [result["intent"], *records]
        for record in records:
            if not isinstance(record, dict):
                continue
            intent_id = record.get("id") or record.get("intent_id")
            if not intent_id or not record.get("date"):
                continue
            citations.setdefault(intent_id, CitedIntent(
                intent_id=intent_id,
                date=record["date"],
                label=str(record.get("label", "")),
                summary=str(record.get("summary", "")),
            ))


async def not_configured_response() -> CopilotNotConfigured | CopilotQueryResponse:
    return CopilotNotConfigured()
