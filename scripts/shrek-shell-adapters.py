#!/usr/bin/env python3
# shrek-shell-adapters.py — ADD the narrow Shrek adapters to the hardened mycolink-shell derivative
# (ADR-006, owner directive 2026-09-02). Removal is done by harden-mycolink-shell.py; this step only ADDS:
#   (1) a file-backed system prompt  — load /usr/share/shrek/ai/system-prompt.md (operator-overridable)
#   (2) the Shrek Memory API recall adapter — repoint the recall transport at the on-box loopback service
# The loopback model-endpoint config is RETAINED as-is (env vars, model_client) — not touched here.
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
