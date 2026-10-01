# `cache push`: upload to the project's binary cache what this machine
# built for CI, so a CI run downloads it instead of building it.
#
# CI builds every flake check, the `ci` dev shell, and the four test
# archives. This command instantiates the same set, takes every output
# in their build closures that exists here and that cache.nixos.org did
# not sign (that is, what was built here rather than downloaded from
# it), and pushes those. Cachix uploads a path's whole closure and
# skips what cache.nixos.org already serves.
#
# Two kinds of path never go up: the test archives (4 to 7 GB each,
# different on every commit) and Claude Code, which the sandbox guest
# carries and is not ours to redistribute. Because Cachix uploads
# closures, any path whose closure contains one of them stays here too.
#
# Arguments: `--dry-run` stops after the summary; `--yes` pushes without
# asking. The push needs a Cachix token with write access to the cache:
# `CACHIX_AUTH_TOKEN`, or one stored with `cachix authtoken`. Without
# either, an interactive run asks for one, without echoing it, and
# stores it for the next run.
#
# Environment: WCMP_CACHE (the cache's name) and WCMP_SYSTEM (the Nix
# system to push for), both set by the menu command.

cache=${WCMP_CACHE:?}
system=${WCMP_SYSTEM:?}

dry=""
yes=""
for argument in "$@"; do
  case $argument in
    --dry-run) dry=1 ;;
    --yes | -y) yes=1 ;;
    *)
      echo "cache push: $argument is not an argument of this command (only --dry-run and --yes)" >&2
      exit 2
      ;;
  esac
done

cd "$(git rev-parse --show-toplevel)"

# The names CI's push filter names too (.github/actions/prepare-runner).
exclude='-tests-(native|web)-(debug|release)-|claude-code'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "cache push: instantiating every check, the ci shell, and the test archives ($system)"
targets=()
while read -r check; do
  targets+=(".#checks.$system.$check")
done < <(nix eval --accept-flake-config --raw ".#checks.$system" \
  --apply 'c: builtins.concatStringsSep "\n" (builtins.attrNames c)')
targets+=(
  ".#devShells.$system.ci"
  .#tests-native-debug
  .#tests-native-release
  .#tests-web-debug
  .#tests-web-release
)
nix path-info --accept-flake-config --derivation "${targets[@]}" > "$work/drvs"

# Every output in their build closures that is valid here.
xargs nix-store --query --requisites --include-outputs < "$work/drvs" \
  | grep -v '\.drv$' | sort -u > "$work/closure"
nix path-info --json --stdin < "$work/closure" > "$work/info.json"

# Built here: no signature from cache.nixos.org. (A path downloaded from
# another cache carries that cache's signature and is pushed too; Cachix
# skips what the cache already holds.)
jq -r '
  (if type == "array" then .[] | {key: .path, value: .} else to_entries[] end)
  | select(.value != null)
  | select([(.value.signatures // .value.sigs // [])[] | startswith("cache.nixos.org-1:")] | any | not)
  | .key | if startswith("/") then . else "/nix/store/" + . end
' "$work/info.json" | sort -u > "$work/built"

# Never pushed: the excluded paths and everything that refers to them.
grep -E -- "$exclude" "$work/closure" > "$work/blocked" || true
if [ -s "$work/blocked" ]; then
  xargs nix-store --query --referrers-closure < "$work/blocked" | sort -u > "$work/tainted"
else
  : > "$work/tainted"
fi
comm -23 "$work/built" "$work/tainted" > "$work/push"

# What Cachix will upload is the closure of the list; hold it to the
# same rule before anything leaves this machine.
if xargs nix-store --query --requisites < "$work/push" | grep -E -- "$exclude"; then
  echo "cache push: the closure above names an excluded path; nothing was pushed" >&2
  exit 1
fi

count=$(wc -l < "$work/push")
held=$(comm -12 "$work/built" "$work/tainted" | wc -l)
if [ "$count" -eq 0 ]; then
  echo "cache push: nothing built here to push; run \`lint\` and \`tests all\` first"
  exit 0
fi
size=$(nix path-info --json --stdin < "$work/push" | jq '
  [(if type == "array" then .[] else to_entries[] | .value end) | .narSize] | add
  | . / 1073741824 | . * 10 | floor / 10')
echo "cache push: $count paths, ${size} GiB before compression; $held held back (test archives, Claude Code, and what contains them)"
if [ "$held" -gt 0 ]; then
  echo "cache push: held back:"
  comm -12 "$work/built" "$work/tainted" | sed 's|^/nix/store/[a-z0-9]*-|  |'
fi
echo "cache push: the largest:"
nix path-info --size --stdin < "$work/push" | sort -k2 -n | tail -n 8 \
  | awk '{ printf "  %8.1f MiB  %s\n", $2 / 1048576, substr($1, 45) }'

if [ -n "$dry" ]; then
  echo "cache push: --dry-run, so nothing was pushed"
  exit 0
fi

config=${XDG_CONFIG_HOME:-$HOME/.config}/cachix/cachix.dhall
if [ -z "${CACHIX_AUTH_TOKEN:-}" ] && ! grep -q 'authToken = "[^"]' "$config" 2>/dev/null; then
  if [ ! -t 0 ]; then
    echo "cache push: no Cachix token; set CACHIX_AUTH_TOKEN or run \`cache push\` in a terminal" >&2
    exit 1
  fi
  read -rsp "Cachix auth token with write access to $cache: " token
  echo
  if [ -z "$token" ]; then
    echo "cache push: no token given; nothing was pushed" >&2
    exit 1
  fi
  printf '%s' "$token" | cachix authtoken --stdin
  unset token
fi

if [ -z "$yes" ]; then
  if [ ! -t 0 ]; then
    echo "cache push: not a terminal, so pass --yes to push without asking" >&2
    exit 1
  fi
  read -rp "Push these $count paths to $cache? [y/N] " answer
  case $answer in
    y | Y | yes) ;;
    *)
      echo "cache push: nothing was pushed"
      exit 0
      ;;
  esac
fi

cachix push "$cache" < "$work/push"
echo "cache push: pushed $count paths to https://$cache.cachix.org"
