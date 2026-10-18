#!/bin/bash
# pruefgeruest-tests.sh – Integrationstests von qs_transaktion gegen SW-1s Prüfgerüst.
#
# Startet je Backend einen eigenen privaten D-Bus (pk-pruefbus.sh von SW-1) unter
# $QS_PK_DIR (Standard /tmp/qs-pk-sw4, kurz wegen der 107-Zeichen-Grenze für
# Unix-Sockets; getrennt von SW-1s /tmp/qs-pk-$USER) und führt die passenden
# #[ignore]-Tests aus qs-cli/tests/pruefgeruest.rs aus. Nie Systembus, nie apt.
#
#   skripte/pruefgeruest-tests.sh            alle Läufe
#   skripte/pruefgeruest-tests.sh attrappe   nur einen Lauf (attrappe, verweigert,
#                                            daemonweg, testfail, testsucceed)
set -euo pipefail
cd "$(dirname "$0")/.."
PK="${QS_PK_WURZEL:-$HOME/projects/querencia-software-paketkit}"
BUS="$PK/pruefgeruest/pk-pruefbus.sh"
export QS_PK_DIR="${QS_PK_DIR:-/tmp/qs-pk-sw4}"
export QS_PK_ATTRAPPE="${QS_PK_ATTRAPPE:-$PK/target/debug/qs-pk-attrappe}"
mkdir -p target
export QS_LOG="${QS_LOG:-$PWD/target/pruefgeruest-software.log}"
ERGEBNIS="$PWD/target/pruefgeruest-ergebnis.txt"
: > "$ERGEBNIS"

[ -x "$BUS" ] || { echo "BLOCKED: $BUS fehlt (SW-1-Prüfgerüst)" >&2; exit 1; }
[ -x "$QS_PK_ATTRAPPE" ] || { echo "BLOCKED: $QS_PK_ATTRAPPE fehlt (in SW-1: cargo build --offline --bin qs-pk-attrappe)" >&2; exit 1; }

stoppen() { "$BUS" stop >/dev/null 2>&1 || true; }
trap stoppen EXIT

nice cargo test --offline -j2 -p qs-cli --test pruefgeruest --no-run

lauf() { # $1 = Name, $2 = Backend, $3 = polkit, $4… = Testfilter
  local name="$1" backend="$2" polkit="$3"; shift 3
  echo "=== $name: Backend $backend, polkit $polkit ==="
  "$BUS" start "$backend" --polkit "$polkit"
  eval "$("$BUS" env)"
  export QS_PK_BUS
  set +e
  nice cargo test --offline -j2 -p qs-cli --test pruefgeruest -- --ignored --test-threads=1 "$@" 2>&1 \
    | tee "target/pruefgeruest-$name.txt"
  local rc=${PIPESTATUS[0]}
  set -e
  echo "$name: $(grep -E '^test result' "target/pruefgeruest-$name.txt" | tail -1) (rc=$rc)" >> "$ERGEBNIS"
  stoppen
  return 0
}

nur="${1:-alle}"
[ "$nur" = alle ] || [ "$nur" = attrappe ]    && lauf attrappe attrappe erlauben attrappe_
[ "$nur" = alle ] || [ "$nur" = verweigert ]  && lauf verweigert attrappe verweigern verweigert_
[ "$nur" = alle ] || [ "$nur" = daemonweg ]   && { lauf daemonweg-ohne attrappe erlauben daemonweg_p15; lauf daemonweg-mit attrappe erlauben daemonweg_p16; }
[ "$nur" = alle ] || [ "$nur" = testfail ]    && lauf testfail test_fail erlauben testfail_
[ "$nur" = alle ] || [ "$nur" = testsucceed ] && lauf testsucceed test_succeed erlauben testsucceed_

echo; echo "=== Zusammenfassung ==="; cat "$ERGEBNIS"
! grep -qE "FAILED|rc=[1-9]" "$ERGEBNIS"
