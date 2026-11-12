# Update rehearsal helpers for the Mac (s115). Sourced by mac-update-rehearsal.yml.

SERVED="$RUNNER_TEMP/served"
REQUESTS="$RUNNER_TEMP/requests.log"
APP=/Applications/Zaaheen.app
LOGDIR="$HOME/Library/Logs/com.zaaheen.app"
PENDING="$HOME/Library/Application Support/com.zaaheen.app/updates/pending.update"
AX=.github/rehearsal/mac-ax.js
LOGMARK=0

write_manifest() { # version url sigfile
  mkdir -p "$SERVED/updates"
  jq -n --arg v "$1" --arg u "$2" --arg s "$(cat "$3")" \
    '{version: $v, notes: "rehearsal", pub_date: (now | todate),
      platforms: {"darwin-aarch64": {url: $u, signature: $s}}}' > "$SERVED/updates/latest.json"
  echo "manifest: $1 -> $2"
}

app_log() { cat "$LOGDIR"/* 2>/dev/null; }

# The log carries over between scenarios: wait_log reads only what this
# start of the app wrote.
start_app() {
  LOGMARK=$(app_log | wc -c | tr -d ' ')
  open -a "$APP"
  echo "opened Zaaheen $(installed_version)"
}

stop_app() {
  osascript -e 'quit app id "com.zaaheen.app"' >/dev/null 2>&1 || true
  sleep 3
  pkill -x zaaheen-desktop || true
  pkill -x zaaheen || true
  pkill -x zaaheen-maintenance || true
  sleep 2
}

# Prints the first pattern seen in this start's log lines, or nothing.
wait_log() { # seconds pattern...
  local end=$((SECONDS + $1)); shift
  while [ $SECONDS -lt $end ]; do
    local new
    new=$(app_log | tail -c +$((LOGMARK + 1)))
    for p in "$@"; do
      if printf '%s' "$new" | grep -qF "$p"; then echo "$p"; return 0; fi
    done
    sleep 5
  done
}

show_log() {
  echo '--- update lines in the app log ---'
  app_log | grep 'update' | tail -20
  echo '--- requests to the stand-in server ---'
  tail -20 "$REQUESTS" 2>/dev/null || true
}

installed_version() {
  /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist" 2>/dev/null
}

ax() { osascript -l JavaScript "$AX" "$@"; }
