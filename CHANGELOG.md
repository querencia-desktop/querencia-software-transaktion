# Änderungsprotokoll

Format nach [Keep a Changelog](https://keepachangelog.com/de/1.1.0/).

## [0.1.1] – 2026-10-18

### Behoben
- Prüfgerüst-Skript verlangt die Attrappe erst nach dem Busstart.

## [0.1.0] – 2026-10-18

Erster Stand (Tag `transaktion/0.1.0`).

### Hinzugefügt
- `qs_transaktion`: Zustandsmaschine (Simulation, Bestätigung, Warteschlange, Sperre, Wiederanbindung), Fehlertexte, Protokoll, `AttrappeQuelle`.
- `qs-cli`: Suche, Details, Aktualisierungen, Simulation, Ausführen, laufende Transaktionen.
- Integrationstests gegen das Prüfgerüst von `querencia-software-paketkit`.
