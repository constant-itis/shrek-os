# Shrek OS — glossary

## Agent

An attested subject with `(identity, profile, trust_band)`. The profile is the default-deny capability allow-set; the trust band sets the isolation floor. See [`agents.md`](agents.md).

## Agent-capability model

The rule that an agent receives precise capabilities, not root or a user account. Effective authority is the sealed profile intersected with live grants. The kernel and broker enforce the result.

## Agentd

The unprivileged resolver daemon. It binds identity, checks `caps ⊆ profile`, computes the requested tier from sealed policy, and emits a request for `gatekeeperd` to re-check.

## Blast radius

What the workload can reach inside its box: paths, commands, search scope, export scope, and network egress. This is the capability profile.

## Blast wall

How strong the containment boundary is: T0, T1, T2, or T3.

## Capability profile

A default-deny declaration of what a subject may do. It uses verbs such as `discover`, `read`, `write`, `execute`, `search`, `summarize`, `export`, and `network`.

## Constructor

The mechanism that realizes an effective tier: `proc_plane` for T0, nspawn/mount/net planes for T1, `t2_plane` for T2, and a later microVM constructor for T3.

## Data authority

The bytes and metadata a subject can reach through deterministic access control.

## Gatekeeperd

The privileged broker and wall. It independently re-checks requests against sealed policy, constructs sandboxes, brokers Onion merges, and must fail closed for agent execution.

## Grant

A bounded authority extension approved through the grant protocol. The agent never receives a bearer token; the broker applies the capability directly.

## Onion

The signed system-layer mechanism: dm-verity-authenticated sysext/confext DDIs selected by sealed policy and merged by systemd under a fixed image policy.

## Plane

A separated enforcement surface. The main distinction is availability plane vs agent-execution plane: the OS should keep booting when semantic services are down, but agent execution must fail closed without the broker.

## Semantic authority

What a subject can learn through search, embeddings, summaries, relationships, and generated context. In Shrek it must never exceed data authority.

## Semantic filesystem

The metadata and semantic projection over the real filesystem, built by `swampd`. It is not storage ground truth.

## Swampd

The filesystem intelligence daemon. It indexes only what its own Landlock allow-set lets it read, and query results are caller-scoped.

## T0

Process sandbox: Landlock, seccomp, full namespace set, cgroups v2, no rootfs. Shipped for the T0 matrix cells.

## T1

System container: `systemd-nspawn` with a synthetic root, private users, private network, read-only relocated grants, and optional sealed-profile egress.

## T2

Userspace-kernel sandbox using gVisor `runsc`. Shipped for no-network cells; T2 egress is deferred.

## T3

MicroVM sandbox, planned around libkrun/Firecracker/Kata. Designed, not shipped.

## Tainted-origin action

An action triggered by untrusted content the agent read. It cannot silently self-serve, even if the action is otherwise in profile; it is demoted to a proposal or explicit acknowledgement path.

## Trusted path

The human approval surface that the sandbox cannot spoof or capture. The design floor is a gatekeeper-owned console VT triggered through systemd `SecureAttentionKey`; graphical overlay is a later shell refinement.

## Trust band

The provenance band of code being executed: `T-first`, `T-pinned`, `T-untrust`, `T-hostile`. The broker derives it from sealed evidence; unknown provenance fails high.
