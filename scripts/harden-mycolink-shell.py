#!/usr/bin/env python3
# harden-mycolink-shell.py — derive the ShrekOS AI front-door shell from a mycolink-shell snapshot by
# REMOVING every host-exec / escalation / agent-dispatch / process-spawn primitive from the source tree
# (ADR-006 §6, owner directive 2026-09-02). Structural, at build time: the primitives are physically absent
# from the sealed artifact, not merely unwired. Deterministic (no timestamps/model) so the resulting source
# digest is reproducible Onion provenance.
#
#   usage: harden-mycolink-shell.py <src agent_harness/> <dest dir>
#
# What it does:
#   1. copies the tree
#   2. DELETES the primitive-bearing modules (escalation/dispatch/exec/process-spawn)
#   3. patches the importers/registrations that referenced them, so the shell still imports
#   4. leaves the loopback model-endpoint config (model_client) UNTOUCHED
# The Shrek adapters (Shrek Memory API recall, file-backed system prompt) are added SEPARATELY as
# Shrek-owned overlay files (see the vendor script) — this script only removes.
#
# stdlib only.
import os
import re
import shutil
import sys

# Modules deleted wholesale — pure escalation/dispatch/exec/process-spawn surfaces the front door never needs.
DELETE = [
    "shell/escalate.py",              # /escalate -> subprocess frontier CLI
    "tools/exec_shell.py",            # host-exec tool (bwrap subprocess)
    "tools/escalate_opus.py",         # escalation dispatch
    "tools/dispatch_sonnet.py",       # external-agent dispatch
    "tools/oneplus_workflows.py",     # vendor workflow subprocess
    "tools/dispatch_tool.py",         # agent-dispatch tool (spawns sub-agents)
    "dispatch.py",                    # multi-agent subprocess dispatch
    "cartridge_lifecycle.py",         # runs cartridge hooks as subprocesses
    "_commit.py",                     # git commit subprocess (provenance only; not needed by the shell)
    "cookbook/hwscan.py",             # hardware scan via subprocess (lspci/…)
]


def rip(text, pattern, repl="", flags=re.M):
    return re.sub(pattern, repl, text, flags=flags)


def patch_file(path, fn):
    with open(path, "r", encoding="utf-8") as f:
        src = f.read()
    out = fn(src)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(out)


def harden_agent(src):
    # commit_sha() came from the deleted _commit module — replace the import IN PLACE (after __future__)
    # with a provenance-neutral stub so import order stays valid.
    src = rip(src, r"^from agent_harness\._commit import .*\n",
              "def commit_sha(*_a, **_k):\n    return None  # shrek-harden: provenance module removed\n")
    # drop imports of deleted modules
    src = rip(src, r"^from agent_harness\.tools\.exec_shell import .*\n")
    src = rip(src, r"^from agent_harness\.tools\.dispatch_sonnet import .*\n")
    src = rip(src, r"^from agent_harness\.tools\.escalate_opus import .*\n")
    src = rip(src, r"^from agent_harness\.tools\.dispatch_tool import .*\n")
    # drop their TOOL_FACTORIES entries
    src = rip(src, r'^\s*"exec_shell":\s*make_exec_shell_tool,\n')
    src = rip(src, r'^\s*"dispatch_sonnet":\s*make_dispatch_sonnet_tool,\n')
    src = rip(src, r'^\s*"escalate_opus":\s*make_escalate_opus_tool,\n')
    src = rip(src, r'^\s*"dispatch_tool":\s*make_dispatch_tool,\n')
    # drop the make_dispatch_tool(...) registration line (used only by the removed dispatch path)
    src = rip(src, r"^\s*make_dispatch_tool\([^\n]*\n")
    # drop exec_shell from any default agent tool list
    src = rip(src, r'("tools":\s*\[[^\]]*?),\s*"exec_shell"', r"\1")
    return src


def harden_repl(src):
    src = rip(src, r"^from \.escalate import .*\n")
    # _escalate_budget(): keep a harmless stub if referenced, else remove the def
    src = rip(src, r"^def _escalate_budget\(\)[\s\S]*?\n(?=\ndef |\nclass |\Z)")
    # _run_escalate(): remove the whole function
    src = rip(src, r"^def _run_escalate\(session[\s\S]*?\n(?=\ndef |\nclass |\Z)")
    # the turn wiring that calls _run_escalate -> replace with a refusal string
    src = rip(
        src,
        r"if result\.should_escalate:\s*\n\s*reply = _run_escalate\(session, result\.escalate_prompt\)",
        'if getattr(result, "should_escalate", False):\n'
        '                    reply = "[escalation is disabled on this ShrekOS build — the model has no '
        'host-exec surface; actions route through the Shrek Tool Contract.]"',
    )
    return src


def harden_substrate_tools(src):
    src = rip(src, r"^\s*registry\.register\(_escalate_spec\(session\)\)\n")
    # remove the _escalate_spec function (spans to next top-level def)
    src = rip(src, r"^def _escalate_spec\(session[\s\S]*?\n(?=\ndef |\nclass |\Z)")
    return src


def harden_commands(src):
    # neuter the /escalate command handler -> a refusal, keep the dataclass fields harmless
    src = rip(
        src,
        r'if cmd == "escalate":\s*\n[\s\S]*?should_escalate=True,\s*\n\s*escalate_prompt=prompt,\s*\n(\s*\))',
        'if cmd == "escalate":\n'
        '        return CommandResult(message="escalation is disabled on this ShrekOS build.")',
    )
    return src


def main():
    if len(sys.argv) != 3:
        sys.stderr.write("usage: harden-mycolink-shell.py <src agent_harness/> <dest dir>\n")
        return 2
    src, dest = sys.argv[1], sys.argv[2]
    if os.path.exists(dest):
        shutil.rmtree(dest)
    shutil.copytree(src, dest, ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "tests", "test_*"))

    for rel in DELETE:
        p = os.path.join(dest, rel)
        if os.path.exists(p):
            os.remove(p)
            print("  deleted", rel)

    patch_file(os.path.join(dest, "agent.py"), harden_agent)
    patch_file(os.path.join(dest, "shell", "repl.py"), harden_repl)
    patch_file(os.path.join(dest, "shell", "substrate_tools.py"), harden_substrate_tools)
    patch_file(os.path.join(dest, "shell", "commands.py"), harden_commands)
    print("  patched agent.py, shell/{repl,substrate_tools,commands}.py")
    return 0


if __name__ == "__main__":
    sys.exit(main())
