#!/usr/bin/env python3
# shrek-shell-adapters.py — ADD the narrow Shrek adapters to the hardened mycolink-shell derivative
# (ADR-006, owner directive 2026-09-02). Removal is done by harden-mycolink-shell.py; this step only ADDS:
#   (1) a file-backed system prompt  — load /usr/share/shrek/ai/system-prompt.md (operator-overridable)
#   (2) the Shrek Memory API recall adapter — repoint the recall transport at the on-box loopback service
#   (3) the ratified reasoning-mode default — flip model_client's enable_thinking default to False so the
#       shipped shell drives the reference model in NORMAL mode by default (ADR-006 §9c reference-model).
# The loopback model-endpoint config (env vars, endpoints) is RETAINED as-is — only the reasoning default
# is aligned here.
#
#   usage: shrek-shell-adapters.py <dest agent_harness/ (already hardened)>
# stdlib only, deterministic.
import os
import re
import sys

# The Shrek Memory API recall adapter PATCHES tools/mycelium_recall.py in place — keeping the module's whole
# public surface (SUBSTRATE_KIND, HARDCODED_FALLBACK_URL, _resolve_url, make_mycelium_recall_tool, …, which
# the sibling mycelium_{save,forget,consolidate} tools import) and swapping ONLY the transport: the two
# functions the recall path uses (_mcp_handshake_and_call, _extract_recall_text) now speak the plain-HTTP
# on-box Shrek Memory API (POST {url}/recall|/save) instead of MCP streamable-http, and the fallback URL
# points at the loopback service. The shell code is otherwise unchanged; the model-endpoint config is
# untouched.
LOOPBACK_URL = "http://127.0.0.1:8199"

SHREK_HANDSHAKE = '''def _mcp_handshake_and_call(url, method, params, timeout=20.0):
    """Shrek Memory API transport (ADR-006 §3/§4) — plain HTTP to the on-box loopback service."""
    import json as _json
    import urllib.request as _urlreq
    base = (url or HARDCODED_FALLBACK_URL).rstrip("/")
    if base.endswith("/mcp"):
        base = base[:-4]
    if method == "save":
        body = {"content": params.get("content", ""), "mtype": params.get("mtype", ""),
                "project": params.get("project", "")}
        endpoint = base + "/save"
    else:
        body = {"query": params.get("query", ""), "limit": int(params.get("limit", 5) or 5)}
        endpoint = base + "/recall"
    req = _urlreq.Request(endpoint, data=_json.dumps(body).encode("utf-8"),
                          headers={"Content-Type": "application/json"})
    with _urlreq.urlopen(req, timeout=timeout) as resp:
        return _json.loads(resp.read().decode("utf-8"))
'''

SHREK_EXTRACT = '''def _extract_recall_text(envelope) -> str:
    """Format a Shrek Memory API response ({memories:[...]} or {id}) into text."""
    if not isinstance(envelope, dict):
        return str(envelope)
    if "id" in envelope and "memories" not in envelope:
        return "saved memory #%s" % envelope.get("id")
    mems = envelope.get("memories", [])
    if not mems:
        return "No memories matched."
    lines = ["## Recall: %d memories" % len(mems)]
    for m in mems:
        tag = m.get("mtype") or m.get("source_type") or ""
        lines.append("  * [%s] %s" % (tag, m.get("content", "")))
    return "\\n".join(lines)
'''


def _replace_func(src, name, new_def):
    # Splice by span (NOT re.sub) — re.sub would interpret backslash escapes in the replacement body.
    pat = r"^def " + re.escape(name) + r"\([\s\S]*?\n(?=\ndef |\nclass |\Z)"
    m = re.search(pat, src, re.M)
    if not m:
        raise SystemExit("shrek-shell-adapters: could not locate %s() to patch" % name)
    return src[:m.start()] + new_def + "\n" + src[m.end():]


def patch_recall(dest):
    p = os.path.join(dest, "tools", "mycelium_recall.py")
    with open(p, "r", encoding="utf-8") as f:
        src = f.read()
    src = re.sub(r'^HARDCODED_FALLBACK_URL\s*=.*$',
                 'HARDCODED_FALLBACK_URL = "%s"  # shrek-adapter: on-box Shrek Memory API' % LOOPBACK_URL,
                 src, count=1, flags=re.M)
    src = _replace_func(src, "_mcp_handshake_and_call", SHREK_HANDSHAKE)
    src = _replace_func(src, "_extract_recall_text", SHREK_EXTRACT)
    with open(p, "w", encoding="utf-8", newline="\n") as f:
        f.write(src)

# (3) reasoning-mode default. The reference model (Granite 4.2 3B, ADR-006 §9c) is a REASONING model:
# with thinking ON its short turns exhaust max_tokens on chain-of-thought and return empty content
# (the #638 trap). The ratified reference-model spec defines NORMAL mode as enable_thinking=false, so the
# shipped shell must DEFAULT to normal — callers that want CoT (e.g. cartridge/agentic turns in repl.py)
# pass enable_thinking=True explicitly. Upstream model_client ships True defaults tuned for the EVO-X2 35B;
# we flip the two public entry points (chat, complete_code) to False for the ShrekOS derivative.
MC_DEFAULT_OLD = "        enable_thinking: bool = True,\n"
MC_DEFAULT_NEW = (
    "        enable_thinking: bool = False,  # shrek-adapter: ratified NORMAL mode (ADR-006 §9c) —"
    " reasoning model returns empty content on short thinking-on turns; callers pass True explicitly\n"
)
MC_DOC_CODE_OLD = "        For code generation we DEFAULT enable_thinking=True. The first\n"
MC_DOC_CODE_NEW = (
    "        ShrekOS default enable_thinking=False (ratified NORMAL mode, ADR-006 §9c);\n"
    "        callers pass enable_thinking=True explicitly when they want CoT. The first\n"
)
MC_DOC_CHAT_OLD = "        Default enable_thinking=True for the agent loop — planning is\n"
MC_DOC_CHAT_NEW = (
    "        ShrekOS default enable_thinking=False (ratified NORMAL mode, ADR-006 §9c);\n"
    "        callers enable thinking explicitly for cartridge/agentic turns. Planning is\n"
)


def patch_model_client(dest):
    p = os.path.join(dest, "model_client.py")
    with open(p, "r", encoding="utf-8") as f:
        src = f.read()
    n = src.count(MC_DEFAULT_OLD)
    if n != 2:
        raise SystemExit(
            "shrek-shell-adapters: expected 2 `enable_thinking: bool = True` defaults in "
            "model_client.py, found %d (source-pin drift?)" % n
        )
    src = src.replace(MC_DEFAULT_OLD, MC_DEFAULT_NEW)
    # Keep the docstrings from contradicting the flipped default (audited sealed tree).
    for old, new in ((MC_DOC_CODE_OLD, MC_DOC_CODE_NEW), (MC_DOC_CHAT_OLD, MC_DOC_CHAT_NEW)):
        if src.count(old) != 1:
            raise SystemExit("shrek-shell-adapters: model_client docstring anchor not unique: %r" % old)
        src = src.replace(old, new, 1)
    with open(p, "w", encoding="utf-8", newline="\n") as f:
        f.write(src)


PROMPT_HOOK = (
    '\n\n# shrek-adapter: file-backed system prompt (ADR-006 §5 iii). MYCOLINK_SYSTEM_PROMPT_FILE points at\n'
    '# /usr/share/shrek/ai/system-prompt.md (sealed default); an operator copy on /home overrides it.\n'
    'import os as _os_shrek\n'
    '_shrek_prompt_file = _os_shrek.environ.get("MYCOLINK_SYSTEM_PROMPT_FILE")\n'
    'if _shrek_prompt_file and _os_shrek.path.exists(_shrek_prompt_file):\n'
    '    with open(_shrek_prompt_file, encoding="utf-8") as _f_shrek:\n'
    '        SHELL_SYSTEM_PROMPT = _f_shrek.read()\n'
)

PROMPT_ANCHOR = '    "re-paste it verbatim."\n)\n'


def main():
    if len(sys.argv) != 2:
        sys.stderr.write("usage: shrek-shell-adapters.py <dest agent_harness/>\n")
        return 2
    dest = sys.argv[1]

    # (1) recall adapter — patch the transport in place (keeps the module's public surface)
    patch_recall(dest)
    print("  patched Shrek Memory API transport into tools/mycelium_recall.py")

    # (3) reasoning-mode default — flip model_client's enable_thinking default to normal mode
    patch_model_client(dest)
    print("  flipped model_client enable_thinking default -> False (ratified NORMAL mode)")

    # (2) file-backed system prompt — append the override after the SHELL_SYSTEM_PROMPT constant
    repl_path = os.path.join(dest, "shell", "repl.py")
    with open(repl_path, "r", encoding="utf-8") as f:
        src = f.read()
    if PROMPT_ANCHOR not in src:
        sys.stderr.write("shrek-shell-adapters: system-prompt anchor not found in repl.py\n")
        return 3
    src = src.replace(PROMPT_ANCHOR, PROMPT_ANCHOR + PROMPT_HOOK, 1)
    with open(repl_path, "w", encoding="utf-8", newline="\n") as f:
        f.write(src)
    print("  installed file-backed system-prompt hook -> shell/repl.py")
    return 0


if __name__ == "__main__":
    sys.exit(main())
