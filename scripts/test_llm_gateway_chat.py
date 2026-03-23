#!/usr/bin/env python3
"""Exercise the local StaticFlow Codex gateway from a single CLI tool."""

import argparse
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path


DEFAULT_BASE_URL = "http://127.0.0.1:3000/api/llm-gateway"
DEFAULT_API_KEY_FILE = "/tmp/staticflow-llm-gateway-e2e/key-created-latest.json"
DEFAULT_MODEL = "gpt-5.3-codex"
ENDPOINT_CHOICES = ["chat", "responses", "responses-compact", "models"]


def load_api_key(explicit_key: str | None, key_file: str) -> str:
    """Load the API key from an explicit value or a helper JSON file."""
    if explicit_key:
        return explicit_key.strip()

    payload = json.loads(Path(key_file).read_text(encoding="utf-8"))
    secret = str(payload.get("secret", "")).strip()
    if not secret:
        raise ValueError(f"`secret` not found in {key_file}")
    return secret


def read_message(positional_message: list[str]) -> str:
    """Read the user prompt from argv or an interactive stdin prompt."""
    if positional_message:
        message = " ".join(positional_message).strip()
    else:
        message = input("Message> ").strip()
    if not message:
        raise ValueError("message is required")
    return message


def build_headers(api_key: str, conversation_id: str | None) -> dict[str, str]:
    """Build the minimum headers required by the gateway."""
    headers = {
        "Authorization": f"Bearer {api_key}",
    }
    if conversation_id:
        headers["conversation_id"] = conversation_id
    return headers


def build_chat_payload(
    model: str,
    message: str,
    stream: bool,
    reasoning_effort: str | None,
    web_search: bool,
    system_prompt: str | None,
) -> dict:
    """Build an OpenAI-style chat/completions payload."""
    messages: list[dict[str, str]] = []
    if system_prompt:
        messages.append({"role": "system", "content": system_prompt})
    messages.append({"role": "user", "content": message})
    payload: dict[str, object] = {
        "model": model,
        "messages": messages,
        "stream": stream,
    }
    if reasoning_effort:
        payload["reasoning_effort"] = reasoning_effort
    if web_search:
        payload["tools"] = [{"type": "web_search"}]
        payload["tool_choice"] = "auto"
    return payload


def build_responses_payload(
    model: str,
    message: str,
    stream: bool | None,
    reasoning_effort: str | None,
    web_search: bool,
    system_prompt: str | None,
) -> dict:
    """Build an OpenAI-style responses payload."""
    payload: dict[str, object] = {
        "model": model,
        "input": message,
    }
    if stream is not None:
        payload["stream"] = stream
    if system_prompt:
        payload["instructions"] = system_prompt
    if reasoning_effort:
        payload["reasoning"] = {"effort": reasoning_effort}
    if web_search:
        payload["tools"] = [{"type": "web_search"}]
        payload["tool_choice"] = "auto"
    return payload


def build_request(
    base_url: str,
    api_key: str,
    endpoint: str,
    model: str,
    stream: bool,
    reasoning_effort: str | None,
    web_search: bool,
    system_prompt: str | None,
    conversation_id: str | None,
    message: str | None,
) -> urllib.request.Request:
    """Create the concrete HTTP request for the selected gateway endpoint."""
    headers = build_headers(api_key, conversation_id)
    base = base_url.rstrip("/")

    if endpoint == "models":
        return urllib.request.Request(
            url=f"{base}/v1/models",
            headers=headers,
            method="GET",
        )

    if message is None:
        raise ValueError("message is required for this endpoint")

    headers["Content-Type"] = "application/json"
    if endpoint == "chat":
        payload = build_chat_payload(
            model=model,
            message=message,
            stream=stream,
            reasoning_effort=reasoning_effort,
            web_search=web_search,
            system_prompt=system_prompt,
        )
        path = "/v1/chat/completions"
    elif endpoint == "responses":
        payload = build_responses_payload(
            model=model,
            message=message,
            stream=stream,
            reasoning_effort=reasoning_effort,
            web_search=web_search,
            system_prompt=system_prompt,
        )
        path = "/v1/responses"
    elif endpoint == "responses-compact":
        payload = build_responses_payload(
            model=model,
            message=message,
            stream=None,
            reasoning_effort=reasoning_effort,
            web_search=web_search,
            system_prompt=system_prompt,
        )
        path = "/v1/responses/compact"
    else:
        raise ValueError(f"unsupported endpoint: {endpoint}")

    body = json.dumps(payload).encode("utf-8")
    return urllib.request.Request(
        url=f"{base}{path}",
        data=body,
        headers=headers,
        method="POST",
    )


def extract_usage(response_json: dict) -> dict | None:
    """Extract the top-level usage block when present."""
    usage = response_json.get("usage")
    return usage if isinstance(usage, dict) else None


def nested_usage_int(value: object, *path: str) -> int | None:
    """Safely read an integer leaf from a nested usage object."""
    current = value
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    if isinstance(current, bool):
        return None
    if isinstance(current, int):
        return current
    return None


def collect_usage_token_fields(value: object, prefix: str = "") -> list[tuple[str, int]]:
    """Collect any token-shaped numeric fields for debugging output."""
    if not isinstance(value, dict):
        return []

    fields: list[tuple[str, int]] = []
    for key, child in value.items():
        full_key = f"{prefix}.{key}" if prefix else key
        if isinstance(child, dict):
            fields.extend(collect_usage_token_fields(child, full_key))
            continue
        if isinstance(child, bool):
            continue
        if isinstance(child, int) and "token" in full_key:
            fields.append((full_key, child))
    return fields


def build_usage_summary_lines(usage: dict) -> list[str]:
    """Normalize common usage shapes into a compact human-readable summary."""
    lines: list[str] = []
    input_tokens = nested_usage_int(usage, "input_tokens")
    if input_tokens is None:
        input_tokens = nested_usage_int(usage, "prompt_tokens")
    output_tokens = nested_usage_int(usage, "output_tokens")
    if output_tokens is None:
        output_tokens = nested_usage_int(usage, "completion_tokens")
    total_tokens = nested_usage_int(usage, "total_tokens")
    cached_tokens = nested_usage_int(usage, "input_tokens_details", "cached_tokens")
    if cached_tokens is None:
        cached_tokens = nested_usage_int(usage, "prompt_tokens_details", "cached_tokens")
    reasoning_tokens = nested_usage_int(usage, "output_tokens_details", "reasoning_tokens")
    if reasoning_tokens is None:
        reasoning_tokens = nested_usage_int(usage, "completion_tokens_details", "reasoning_tokens")

    if input_tokens is not None:
        lines.append(f"input_tokens: {input_tokens}")
    if cached_tokens is not None:
        lines.append(f"input_cached_tokens: {cached_tokens}")
    if input_tokens is not None and cached_tokens is not None:
        lines.append(f"input_uncached_tokens: {max(input_tokens - cached_tokens, 0)}")
    if output_tokens is not None:
        lines.append(f"output_tokens: {output_tokens}")
    if reasoning_tokens is not None:
        lines.append(f"reasoning_tokens: {reasoning_tokens}")
    if total_tokens is not None:
        lines.append(f"total_tokens: {total_tokens}")

    seen = {line.split(": ", 1)[0] for line in lines}
    for key, value in sorted(collect_usage_token_fields(usage)):
        if key in {
            "input_tokens",
            "prompt_tokens",
            "output_tokens",
            "completion_tokens",
            "total_tokens",
            "input_tokens_details.cached_tokens",
            "prompt_tokens_details.cached_tokens",
            "output_tokens_details.reasoning_tokens",
            "completion_tokens_details.reasoning_tokens",
        }:
            continue
        friendly_key = key.replace("prompt_tokens", "input_tokens").replace(
            "completion_tokens", "output_tokens"
        )
        if friendly_key not in seen:
            lines.append(f"{friendly_key}: {value}")
            seen.add(friendly_key)

    return lines


def print_usage_summary(usage: dict | None) -> None:
    """Print the post-request token summary shown after every invocation."""
    print("\nToken Usage:")
    if usage is None:
        print("  unavailable")
        return
    for line in build_usage_summary_lines(usage):
        print(f"  {line}")


def flatten_response_content(node: object, out: list[str]) -> None:
    """Recursively extract text fragments from chat or responses output items."""
    if isinstance(node, str):
        if node:
            out.append(node)
        return
    if isinstance(node, list):
        for item in node:
            flatten_response_content(item, out)
        return
    if not isinstance(node, dict):
        return

    node_type = node.get("type")
    if node_type in {"text", "input_text", "output_text"}:
        text = node.get("text")
        if isinstance(text, str) and text:
            out.append(text)
        return
    if node_type == "message":
        flatten_response_content(node.get("content"), out)
        return
    if node_type == "content_part":
        flatten_response_content(node.get("text"), out)
        return
    if node_type == "output_item":
        flatten_response_content(node.get("content"), out)
        return
    if node_type == "reasoning":
        summary = node.get("summary")
        if isinstance(summary, list):
            flatten_response_content(summary, out)
        elif isinstance(summary, dict):
            flatten_response_content(summary, out)
        return
    if "content" in node:
        flatten_response_content(node["content"], out)
        return
    if "item" in node:
        flatten_response_content(node["item"], out)
        return
    if "output_item" in node:
        flatten_response_content(node["output_item"], out)
        return
    if "part" in node:
        flatten_response_content(node["part"], out)
        return
    if "content_part" in node:
        flatten_response_content(node["content_part"], out)


def extract_chat_reply(response_json: dict) -> str:
    """Extract the final assistant reply from a chat/completions response."""
    choices = response_json.get("choices")
    if not isinstance(choices, list) or not choices:
        raise ValueError("response missing choices")
    message = choices[0].get("message")
    if not isinstance(message, dict):
        raise ValueError("response missing message")
    content = message.get("content")
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    return json.dumps(content, ensure_ascii=False)


def extract_responses_reply(response_json: dict) -> str:
    """Extract the final text reply from a responses payload."""
    output = response_json.get("output")
    if not isinstance(output, list):
        raise ValueError("response missing output array")
    text_parts: list[str] = []
    for item in output:
        flatten_response_content(item, text_parts)
    return "".join(text_parts).strip()


def extract_compaction_summary(response_json: dict) -> str:
    """Render a human-readable compact result without leaking opaque state."""
    output = response_json.get("output")
    if not isinstance(output, list):
        raise ValueError("response missing output array")
    encrypted_items = 0
    for item in output:
        if not isinstance(item, dict):
            continue
        if item.get("type") == "compaction_summary":
            encrypted_content = item.get("encrypted_content")
            if isinstance(encrypted_content, str) and encrypted_content:
                encrypted_items += 1
    if encrypted_items > 0:
        return f"Compaction completed. encrypted_summaries={encrypted_items}"
    return "Compaction completed."


def extract_response_usage(event_json: dict) -> dict | None:
    """Extract usage from streamed events that may wrap the response object."""
    response = event_json.get("response")
    if isinstance(response, dict):
        usage = response.get("usage")
        if isinstance(usage, dict):
            return usage
    return extract_usage(event_json)


def extract_chat_delta_text(value: dict) -> str:
    """Extract delta text from a streamed chat/completions event."""
    choices = value.get("choices")
    if not isinstance(choices, list) or not choices:
        return ""
    delta = choices[0].get("delta")
    if not isinstance(delta, dict):
        return ""
    content = delta.get("content")
    return content if isinstance(content, str) else ""


def extract_responses_delta_text(event_type: str, value: dict) -> str:
    """Extract delta text from streamed responses events."""
    if event_type == "response.output_text.delta":
        delta = value.get("delta")
        if isinstance(delta, str):
            return delta
        if isinstance(delta, dict):
            text = delta.get("text")
            if isinstance(text, str):
                return text
    return ""


def flush_sse_event(
    event_lines: list[str],
    endpoint: str,
) -> tuple[bool, dict | None, bool, dict | None]:
    """Parse one buffered SSE event and optionally print streamed text."""
    if not event_lines:
        return False, None, False, None

    event_type = ""
    data_lines: list[str] = []
    for line in event_lines:
        if line.startswith("event:"):
            event_type = line[6:].lstrip()
        elif line.startswith("data:"):
            data_lines.append(line[5:].lstrip())
    if not data_lines:
        return False, None, False, None

    payload = "\n".join(data_lines).strip()
    if not payload:
        return False, None, False, None
    if payload == "[DONE]":
        return True, None, False, None

    value = json.loads(payload)
    usage = extract_response_usage(value)
    printed_content = False

    if endpoint == "chat":
        text = extract_chat_delta_text(value)
        if text:
            print(text, end="", flush=True)
            printed_content = True
    else:
        text = extract_responses_delta_text(event_type, value)
        if text:
            print(text, end="", flush=True)
            printed_content = True

    return False, usage, printed_content, value


def stream_reply(
    request: urllib.request.Request,
    endpoint: str,
    show_usage: bool,
    show_raw: bool,
    timeout: int,
) -> int:
    """Consume a streaming gateway response and print text as it arrives."""
    usage = None
    printed_content = False
    last_event = None
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            event_lines: list[str] = []
            while True:
                raw_line = response.readline()
                if not raw_line:
                    if event_lines:
                        _, maybe_usage, did_print, maybe_value = flush_sse_event(
                            event_lines, endpoint
                        )
                        usage = maybe_usage or usage
                        printed_content = printed_content or did_print
                        last_event = maybe_value or last_event
                    break
                line = raw_line.decode("utf-8", errors="replace").rstrip("\r\n")
                if not line:
                    done, maybe_usage, did_print, maybe_value = flush_sse_event(
                        event_lines, endpoint
                    )
                    usage = maybe_usage or usage
                    printed_content = printed_content or did_print
                    last_event = maybe_value or last_event
                    event_lines.clear()
                    if done:
                        break
                    continue
                event_lines.append(line)
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        print(f"HTTP {exc.code}: {detail}", file=sys.stderr)
        return 1
    except Exception as exc:
        print(f"Request failed: {exc}", file=sys.stderr)
        return 1

    if printed_content:
        print()

    print_usage_summary(usage)

    if show_usage and usage is not None:
        print(json.dumps(usage, ensure_ascii=False, indent=2), file=sys.stderr)
    if show_raw and last_event is not None:
        print(json.dumps(last_event, ensure_ascii=False, indent=2), file=sys.stderr)

    return 0


def print_models(payload: dict, show_raw: bool) -> int:
    """Print one available model per line for quick CLI inspection."""
    data = payload.get("data")
    if not isinstance(data, list):
        print("Invalid models payload", file=sys.stderr)
        if show_raw:
            print(json.dumps(payload, ensure_ascii=False, indent=2), file=sys.stderr)
        return 1
    for item in data:
        if not isinstance(item, dict):
            continue
        model_id = item.get("id")
        owned_by = item.get("owned_by")
        if isinstance(model_id, str):
            if isinstance(owned_by, str) and owned_by:
                print(f"{model_id}\t{owned_by}")
            else:
                print(model_id)
    if show_raw:
        print(json.dumps(payload, ensure_ascii=False, indent=2), file=sys.stderr)
    return 0


def parse_args() -> argparse.Namespace:
    """Build the command-line interface for gateway smoke tests."""
    parser = argparse.ArgumentParser(
        description="Exercise the local StaticFlow Codex gateway: list models, call responses, or call chat/completions."
    )
    parser.add_argument(
        "message",
        nargs="*",
        help="message text; required for chat/responses endpoints, omitted for models",
    )
    parser.add_argument(
        "--endpoint",
        choices=ENDPOINT_CHOICES,
        default="chat",
        help="gateway endpoint to call",
    )
    parser.add_argument("--base-url", default=DEFAULT_BASE_URL, help="gateway base url")
    parser.add_argument("--api-key", default=None, help="explicit API key to use")
    parser.add_argument(
        "--api-key-file",
        default=DEFAULT_API_KEY_FILE,
        help="json file containing the temporary key under `secret`",
    )
    parser.add_argument("--model", default=DEFAULT_MODEL, help="model name")
    parser.add_argument(
        "--system",
        default=None,
        help="optional system prompt/instructions",
    )
    parser.add_argument(
        "--conversation-id",
        default=None,
        help="optional conversation_id header for thread stickiness",
    )
    parser.add_argument(
        "--no-stream",
        action="store_true",
        help="disable SSE streaming and wait for the final JSON reply",
    )
    parser.add_argument(
        "--show-usage",
        action="store_true",
        help="also print usage json to stderr",
    )
    parser.add_argument(
        "--show-raw",
        action="store_true",
        help="also print the raw JSON payload to stderr",
    )
    parser.add_argument(
        "--reasoning-effort",
        choices=["low", "medium", "high", "xhigh"],
        default=None,
        help="optional reasoning depth passed through to the gateway",
    )
    parser.add_argument(
        "--web-search",
        action="store_true",
        help="enable the Codex web_search tool for real-time answers",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=300,
        help="request timeout in seconds",
    )
    return parser.parse_args()


def main() -> int:
    """Run one gateway request and print the normalized result."""
    args = parse_args()

    try:
        api_key = load_api_key(args.api_key, args.api_key_file)
        message = None if args.endpoint == "models" else read_message(args.message)
        stream = (not args.no_stream) and args.endpoint not in {"models", "responses-compact"}
        request = build_request(
            base_url=args.base_url,
            api_key=api_key,
            endpoint=args.endpoint,
            model=args.model,
            stream=stream,
            reasoning_effort=args.reasoning_effort,
            web_search=args.web_search,
            system_prompt=args.system,
            conversation_id=args.conversation_id,
            message=message,
        )
        if stream:
            return stream_reply(
                request=request,
                endpoint=args.endpoint,
                show_usage=args.show_usage,
                show_raw=args.show_raw,
                timeout=args.timeout,
            )
        with urllib.request.urlopen(request, timeout=args.timeout) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        print(f"HTTP {exc.code}: {detail}", file=sys.stderr)
        return 1
    except Exception as exc:
        print(f"Request failed: {exc}", file=sys.stderr)
        return 1

    if args.endpoint == "models":
        return print_models(payload, args.show_raw)

    try:
        if args.endpoint == "chat":
            reply = extract_chat_reply(payload)
        elif args.endpoint == "responses-compact":
            reply = extract_compaction_summary(payload)
        else:
            reply = extract_responses_reply(payload)
    except Exception as exc:
        print(f"Failed to parse reply: {exc}", file=sys.stderr)
        print(json.dumps(payload, ensure_ascii=False, indent=2), file=sys.stderr)
        return 1

    print(reply)
    print_usage_summary(extract_usage(payload))

    if args.show_usage:
        usage = extract_usage(payload)
        if usage is not None:
            print(json.dumps(usage, ensure_ascii=False, indent=2), file=sys.stderr)
    if args.show_raw:
        print(json.dumps(payload, ensure_ascii=False, indent=2), file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
