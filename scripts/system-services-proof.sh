#!/usr/bin/env bash
# Direct parser proof for the shell-v2 System service seams. This does not mutate host state; it validates
# the text contracts the QML services consume from mature desktop CLIs.
set -euo pipefail

pass=0
fail=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

ok() { echo "PASS $1"; pass=$((pass + 1)); }
bad() { echo "FAIL $1: $2"; fail=$((fail + 1)); }

cat >"$tmp/wpctl.txt" <<'EOF'
Audio
 ├─ Sinks:
 │  *   51. Built-in Audio Analog Stereo [vol: 0.42]
 │      64. USB-C Dock Audio [vol: 0.30]
 ├─ Sources:
 │  *   73. Built-in Microphone [vol: 0.80]
EOF

wp_out="$(
  awk 'BEGIN{sec=""}
    /Sinks:/{sec="out"; next}
    /Sources:/{sec="in"; next}
    /^[[:alnum:]_ -]+$/{sec=""}
    sec && match($0,/^[^0-9*]*(\*)?[[:space:]]*([0-9]+)\. (.*)$/,m){
      label=m[3]; sub(/[[:space:]]*\[vol:.*$/, "", label);
      printf "%s|%s|%s|%s\n", sec,m[2],label,(m[1]=="*"?"1":"0")
    }' "$tmp/wpctl.txt"
)"

grep -q '^out|51|Built-in Audio Analog Stereo|1$' <<<"$wp_out" && ok audio-default-sink || bad audio-default-sink "$wp_out"
grep -q '^out|64|USB-C Dock Audio|0$' <<<"$wp_out" && ok audio-alt-sink || bad audio-alt-sink "$wp_out"
grep -q '^in|73|Built-in Microphone|1$' <<<"$wp_out" && ok audio-default-source || bad audio-default-source "$wp_out"

cat >"$tmp/nm-active.txt" <<'EOF'
Home WiFi|802-11-wireless|wlan0
EOF
nm_active="$(awk -F'|' 'NR==1{printf "active|%s|%s|%s\n",$1,$2,$3}' "$tmp/nm-active.txt")"
[ "$nm_active" = "active|Home WiFi|802-11-wireless|wlan0" ] && ok nm-active || bad nm-active "$nm_active"

cat >"$tmp/nm-saved.txt" <<'EOF'
Home WiFi|802-11-wireless
Dock Ethernet|802-3-ethernet
EOF
nm_saved="$(awk -F'|' '$2 ~ /802-11-wireless|wifi/{printf "saved|%s\n",$1}' "$tmp/nm-saved.txt")"
[ "$nm_saved" = "saved|Home WiFi" ] && ok nm-saved-wifi || bad nm-saved-wifi "$nm_saved"

cat >"$tmp/nm-wifi.txt" <<'EOF'
*|Home WiFi|87|WPA2
|Cafe Open|44|
EOF
nm_wifi="$(awk -F'|' 'length($2)>0{printf "wifi|%s|%s|%s|%s\n",$1,$2,$3,$4}' "$tmp/nm-wifi.txt")"
grep -q '^wifi|\*|Home WiFi|87|WPA2$' <<<"$nm_wifi" && ok nm-active-ap || bad nm-active-ap "$nm_wifi"
grep -q '^wifi||Cafe Open|44|$' <<<"$nm_wifi" && ok nm-open-ap || bad nm-open-ap "$nm_wifi"

echo "SYSTEM-SERVICES-PROOF: PASS=$pass FAIL=$fail"
[ "$fail" -eq 0 ]
