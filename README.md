# Querencia Software – Transaktionen (SW-4)

Teil von „Querencia Software“, dem grafischen Software-Installer für Querencia Desktop.
Dieses Repository liefert die Schicht zwischen Oberfläche (SW-3) und PackageKit-Schicht (SW-1):

| Crate | Art | Inhalt |
|---|---|---|
| `qs_transaktion` | Bibliothek | `Monitor` (implementiert `Transaktionsmonitor`), Fehlertexte, Protokoll, `AttrappeQuelle` |
| `qs-cli` | Programm | Suche, Details, Aktualisierungen, Simulation und Probeläufe auf der Kommandozeile |

Typen und Traits (`Auftrag`, `Zustand`, `TransaktionsGriff`, `Transaktionsmonitor`,
`Paketquelle`) kommen aus `qs_typen` von SW-1 (`../querencia-software-paketkit/qs_typen`),
verbindlich nach `~/projects/querencia-software/SCHNITTSTELLEN.md`.

## Zustandsmaschine

```text
starten ─► Simuliert ─┬─► WartetAufBestaetigung ─(bestaetigen)─┐
                      │   Entfernen immer; Installieren bei     │
                      │   Zusatzpaketen oder > 50 MiB Download  │
                      └────────────────────────────────────────┤
                                  Warteschlange: eine zur Zeit ▼ in Startreihenfolge
     WartetAufSperre ◄─ SperreBelegt ── Laeuft{prozent,status} ──► Fertig
       alle 10 s erneut, höchstens 5 min;          │     (Neustart: Statustext NEUSTART_HINWEIS
       danach Fehlgeschlagen(SperreBelegt)         │      und Monitor::braucht_neustart)
                                                   ├─► DaemonWeg: ein Wiederanbindungsversuch
                                                   │   (laufende_transaktionen, Zielzustand per
                                                   │   Resolve/GetUpdates, sonst dieselbe Liste
                                                   │   erneut), sonst Fehlgeschlagen(DaemonWeg)
                                                   ├─► KeinAgent / NichtBerechtigt / andere
                                                   │   → sofort Fehlgeschlagen(fehler)
                                                   └─► 10 min ohne Fortschritt → Nachfrage
                                                       (ist_nachfrage), kein Abbruch
  abbrechen() in jedem Zustand ─► Abgebrochen
```

Alle Zeiten stehen in `Einstellungen` und sind einstellbar.

## Einbinden (Oberfläche)

```toml
qs_transaktion = { path = "../querencia-software-transaktion/qs_transaktion", features = ["glib"] }
```

```rust
use std::rc::Rc;
use qs_transaktion::{laufzeit::GlibLaufzeit, Monitor, Transaktionsmonitor};

let monitor = Monitor::neu(Rc::new(GlibLaufzeit));          // einmal je Programm
let griff = monitor.starten(auftrag, quelle.clone(), Box::new(move |zustand| { /* anzeigen */ }));
// griff.bestaetigen() / griff.abbrechen() / griff.sperre_erneut_versuchen()
let text = qs_transaktion::fehlertexte::fehlertext(&fehler); // Überschrift, Erklärung, Handlung
```

## qs-cli

```bash
cargo build --offline -j2 -p qs-cli
```

```bash
target/debug/qs-cli hilfe
```

```bash
target/debug/qs-cli fuehre-aus --attrappe --szenario sperre-2 --schnell installieren vlc
```

Ziele: `--attrappe` (Standard, ändert nichts), `--pruefgeruest` (privater Bus aus `QS_PK_BUS`,
schreibend nur mit Test-Backend), `--echt` (Systembus, **nur lesend**; `fuehre-aus` wird mit
Rückgabewert 3 verweigert).

## Protokoll

`~/.local/state/querencia/software.log` (oder `$XDG_STATE_HOME/querencia/software.log`,
übersteuerbar mit `QS_LOG`, leer = aus). Eine Zeile je Zustands- oder Statuswechsel:
Zeit, Griff-ID, `von → nach`, bei Fehlern der PackageKit-Fehlercode. Paketlisten höchstens
20 Einträge. Rotation nach `software.log.1` ab 1 MB.

## Prüfen

```bash
nice cargo test --offline -j2 --workspace --features qs_transaktion/tokio
```

```bash
nice cargo clippy --offline -j2 --workspace --all-targets --features qs_transaktion/tokio,qs_transaktion/glib -- -D warnings
```

```bash
skripte/pruefgeruest-tests.sh
```

Das Skript startet je Backend einen privaten D-Bus mit SW-1s Prüfgerüst unter `/tmp/qs-pk-sw4`
und führt die `#[ignore]`-Tests aus `qs-cli/tests/pruefgeruest.rs` aus. Es fasst nie den
Systembus oder das apt-Backend an.

Lizenz: GPL-3.0-or-later.
