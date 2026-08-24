import os
import re
import subprocess

import libcalamares


def pretty_name():
    return "Deploy Shrek OS"


def pretty_status_message():
    return "Writing the Shrek OS disk image and initial machine state."


def _gs_value(key, default=None):
    try:
        if libcalamares.globalstorage.contains(key):
            return libcalamares.globalstorage.value(key)
    except Exception:
        pass
    return default


def _parent_disk(device):
    if not device:
        return None
    try:
        out = subprocess.check_output(
            ["lsblk", "-ndo", "PKNAME", device],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
    except subprocess.CalledProcessError:
        out = ""
    if out:
        return "/dev/" + out.splitlines()[0]
    if re.match(r"^/dev/nvme\d+n\d+p\d+$", device):
        return re.sub(r"p\d+$", "", device)
    if re.match(r"^/dev/[a-z]+[0-9]+$", device):
        return re.sub(r"[0-9]+$", "", device)
    return device


def _target_disk_from_partitions():
    parts = _gs_value("partitions", []) or []
    root_part = None
    first_part = None
    for part in parts:
        if not isinstance(part, dict):
            continue
        device = part.get("device")
        if device and first_part is None:
            first_part = device
        if part.get("mountPoint") == "/":
            root_part = device
            break
    return _parent_disk(root_part or first_part)


def run():
    libcalamares.job.setprogress(0.02)

    target_disk = _target_disk_from_partitions()
    username = _gs_value("username", "")
    fullname = _gs_value("fullname", "")
    hostname = _gs_value("hostname", "shrek")

    if not target_disk:
        return (
            "No target disk selected",
            "Calamares did not expose a selected target disk. Return to partitioning and select a disk.",
        )
    if not username:
        return (
            "No user configured",
            "Calamares did not expose a user account. Return to the user page and create the owner account.",
        )

    libcalamares.job.setprogress(0.08)
    cmd = [
        "/usr/libexec/shrek/shrek-install-target",
        "--target-disk",
        target_disk,
        "--username",
        username,
        "--fullname",
        fullname,
        "--hostname",
        hostname,
    ]
    try:
        libcalamares.utils.debug("shrekdeploy: running {}".format(" ".join(cmd)))
        subprocess.check_call(cmd)
    except subprocess.CalledProcessError as exc:
        return (
            "Shrek deployment failed",
            "The Shrek deployment job exited with status {}. See the installer log for details.".format(exc.returncode),
        )

    libcalamares.job.setprogress(1.0)
    return None
