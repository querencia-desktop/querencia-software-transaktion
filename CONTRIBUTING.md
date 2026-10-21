# Mitmachen bei Querencia Software – Transaktionen (qs_transaktion, qs-cli)

Danke für das Interesse. Querencia ist in einer frühen Alpha-Phase; bitte
rechne mit Rückfragen und langsamen Antworten.

Jede Änderung durchläuft den unten beschriebenen Prüf- und Freigabeweg;
pottrauschen (der „Besitzer“) nimmt sie ab.

## Fehler melden

In der gemeinsamen Meldestelle:
<https://github.com/querencia-desktop/querencia/issues/new/choose>.

**Sicherheitslücken nicht öffentlich melden:** siehe [SECURITY.md](SECURITY.md).

## Arbeitsweise

### 1. Isolierte Prüfung

- Nie in der laufenden Sitzung testen. GUI-Prüfungen laufen in einem eigenen
  labwc ohne Bildschirm mit eigenem `HOME`, `XDG_RUNTIME_DIR` und
  `XDG_CONFIG_HOME` (`skripte/`).
- Vor jedem Pull Request, ohne Bildschirm:
  `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo test --locked`.
- Bei Änderungen an der Oberfläche zusätzlich ein isolierter GUI-Lauf in
  1024×768 und 1920×1080 mit Bildschirmfotos als Beleg.
- Jede Behebung eines Fehlers bekommt einen Test, der vorher scheitert.

### 2. Berichte

Jede abgeschlossene Aufgabe hinterlässt:

- einen **Bericht** (was geändert wurde, was geprüft wurde, Zahlen, was *nicht*
  geprüft wurde, Befunde für andere),
- eine **Übergabe** (Zuständigkeit, Ordner, Prüfwerkzeuge, Integration und
  Rückfall, Offenes, Fallen, Berichte),
- Versionen als Git-Tags (`<programm>/<version>`), Berichte nennen Commits.

Im Pull Request stehen Zusammenfassung und Prüfzahlen; Unfertiges wird als
unfertig gekennzeichnet, in Code, Dokumentation und Changelog.

### 3. Freigaben

- Der Besitzer entscheidet über Merge und Release. Nichts gelangt ohne
  ausdrückliche Freigabe in `main`, in ein Paket oder in eine Sitzung.
- Eine Freigabe zählt nur, wenn sie direkt in dem Pull Request bzw. der Aufgabe
  gegeben wird, die sie ausführt; eine weitergeleitete Freigabe gilt nicht.
- Änderungen an laufenden Systemen (Sitzung, Autostart, `rc.xml`, Pakete)
  brauchen immer einen eigenen, vorab beschriebenen Rückfall.

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
   passen und in `LIZENZEN.md` stehen. Nichtfreie Anteile (z. B. UnRAR) sind
   ausgeschlossen.
7. Quellcode und Texte ohne Fremdmaterial: keine Übernahme aus den Vorbild­
   programmen (siehe README, „Vorbild und Abgrenzung“).

## Lizenz der Beiträge

Mit einem Beitrag stimmst du zu, dass er unter der Lizenz dieses Repos
(GPL-3.0-or-later) veröffentlicht wird. Du versicherst, dass du ihn selbst
geschrieben hast oder die Lizenz des Ursprungs es erlaubt.
