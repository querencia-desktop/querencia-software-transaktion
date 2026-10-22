# Mitmachen bei Querencia Software – Transaktionen (qs_transaktion, qs-cli)

Danke für das Interesse. Querencia ist in einer frühen Alpha-Phase; bitte
rechne mit Rückfragen und langsamen Antworten.

## Fehler melden

In der gemeinsamen Meldestelle:
<https://github.com/querencia-desktop/querencia/issues/new/choose>.

**Sicherheitslücken nicht öffentlich melden:** siehe [SECURITY.md](SECURITY.md).

## Vor einem Pull Request

- Integrationstests nur gegen das Prüfgerüst mit eigenem D-Bus
  (`skripte/pruefgeruest-tests.sh`), nie gegen den Systembus, nie mit apt
  oder sudo.

Checkliste:

- [ ] Build: `cargo build --locked`
- [ ] Format: `cargo fmt --all --check`
- [ ] Lint: `cargo clippy --all-targets --locked -- -D warnings`
- [ ] Tests: `cargo test --locked`; jede Fehlerbehebung bekommt einen Test,
      der vorher scheitert
- [ ] Lizenzkopf bzw. Lizenzangabe: neue Dateien stehen unter der Lizenz des
      Repos; Fremdmaterial mit Herkunft und Lizenz in `COPYRIGHT`
- [ ] Keine Geheimnisse (Schlüssel, Passwörter, Tokens), keine privaten Pfade
      oder Adressen
- [ ] `CHANGELOG.md` ergänzt; Unfertiges ist als unfertig gekennzeichnet

Über Merge und Release entscheiden die Betreuer des Projekts.

## Regeln für den Code

1. Vorab ein Issue, wenn es um mehr als eine kleine Korrektur geht –
   besonders bei Bedienung und Optik. Maßstab ist Ubuntu Software Center / gnome-packagekit (Zusammenspiel PackageKit + AppStream).
2. Zweig von `main`, kleine, thematisch geschlossene Commits
   (Conventional Commits).
3. App-IDs, Desktop-Dateien und D-Bus-Namen folgen `org.querencia.*`.
4. Keine fest verdrahteten Pfade (`/home/<name>`, `~/projects`, `~/opt`,
   `target/`), keine privaten Adressen in Code, Tests und Doku.
5. Symbole: nie 48×48 zusammen mit `scalable` installieren (GTK-4.22-Absturz,
   siehe Regel M in `querencia-packaging`).
6. Neue Abhängigkeiten nur mit Begründung; die Lizenz muss zu GPL-3.0-or-later
   passen und in `COPYRIGHT` stehen. Nichtfreie Anteile (z. B. UnRAR) sind
   ausgeschlossen.
7. Quellcode und Texte ohne Fremdmaterial: keine Übernahme aus den Vorbild­
   programmen (siehe README).

## Lizenz der Beiträge

Mit einem Beitrag stimmst du zu, dass er unter der Lizenz dieses Repos
(GPL-3.0-or-later) veröffentlicht wird. Du versicherst, dass du ihn selbst
geschrieben hast oder die Lizenz des Ursprungs es erlaubt.
