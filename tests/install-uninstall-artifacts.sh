#!/usr/bin/env bash
# A missing systemd user session must not leave commands, units, or timer stamps
# behind after uninstall. Keep this black-box: run the installer in an isolated
# HOME and give uninstall a PATH that genuinely has no systemctl or crontab.
set -euo pipefail

root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
h="$(mktemp -d)"
path="$(mktemp -d)"
trap 'rm -rf "$h" "$path"' EXIT

HOME="$h" "$root/install.sh" >/dev/null
mkdir -p "$h/.local/state/sysknife-maint/runs" \
    "$h/.local/share/systemd/timers" "$h/.config/systemd/user"
printf 'keep this audit trail\n' > "$h/.local/state/sysknife-maint/runs/keep.md"
printf 'stamp\n' > "$h/.local/share/systemd/timers/stamp-sysknife-review"
printf '[Service]\n' > "$h/.config/systemd/user/maintainer@sysknife-review.service"
printf '[Timer]\n' > "$h/.config/systemd/user/maintainer@sysknife-review.timer"
printf '[Service]\n' > "$h/.config/systemd/user/podman-userns-warmup.service"

# Model an SSH session where systemctl exists but no user bus answers. Stub
# crontab too so this test can never read or change the host user's crontab.
printf '#!/usr/bin/env bash\nexit 1\n' > "$path/systemctl"
printf '#!/usr/bin/env bash\nexit 0\n' > "$path/crontab"
chmod +x "$path/systemctl" "$path/crontab"

PATH="$path:$PATH" HOME="$h" MAINTAINER_UNINSTALL_YES=1 \
    "$root/install.sh" --uninstall >/dev/null

for artifact in \
    "$h/.local/bin/maintainer-mcp" \
    "$h/.config/systemd/user/maintainer@sysknife-review.service" \
    "$h/.config/systemd/user/maintainer@sysknife-review.timer" \
    "$h/.config/systemd/user/podman-userns-warmup.service" \
    "$h/.local/share/systemd/timers/stamp-sysknife-review"; do
    if [ -e "$artifact" ]; then
        echo "uninstall left $artifact" >&2
        exit 1
    fi
done

if [ ! -f "$h/.local/state/sysknife-maint/runs/keep.md" ]; then
    echo "uninstall removed the audit trail" >&2
    exit 1
fi
unexpected="$(find "$h" -type f ! -path "$h/.local/state/*" -print)"
if [ -n "$unexpected" ]; then
    printf 'uninstall left files outside the audit trail:\n%s\n' "$unexpected" >&2
    exit 1
fi
echo "PASS uninstall removes commands, units, and timer stamps without a systemd user session; keeps audit trail"
