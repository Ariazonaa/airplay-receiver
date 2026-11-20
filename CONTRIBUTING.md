# Contributing

*Deutsche Kurzfassung weiter unten.*

Issues and pull requests are welcome – bug reports, ideas, questions, fixes, translations, documentation. I usually reply within **3 days**.

## Reporting a bug

Open an [issue](https://github.com/Ariazonaa/airplay-receiver/issues) and include:

- What happened and what you expected
- iPhone/iPad model and iOS/iPadOS version
- Windows version and GPU
- Mode (*Low latency* or *Smooth*) and whether H.265 is allowed
- The log file `%LOCALAPPDATA%\airplay\logs\airplay.log` (attach it or paste the relevant part)

Connection problems: please check the firewall rule and the VPN note in the [README](README.md#download--usage) first. `airplay-probe.exe` in the program folder shows whether the iPhone reaches the PC at all.

## Pull requests

1. Fork the repository and create a branch from `main`.
2. Build and test as described under [Building](README.md#building):
   - `scripts/build.sh` (build + Clippy with `-D warnings`) must pass.
   - `cargo test` must pass. On a machine without a hardware GPU or network use `AIRPLAY_CI=1 cargo test`.
3. Keep the change focused; add or adjust tests for changed behaviour.
4. User-facing texts exist in German and English (`src/app/strings.rs`) – please add both. If you only speak one of them, say so in the PR and I will fill in the other.
5. Open the pull request against `main` and describe what changed and why. CI runs automatically.

Larger changes (new features, protocol changes, new dependencies): please open an issue first so we can agree on the approach.

## License

By contributing you agree that your contribution is licensed under the project's license, [GPL-3.0](LICENSE). Code taken from UxPlay (`third_party/uxplay/`) keeps its upstream license; changes there should stay small and be noted in `third_party/uxplay/VENDORED.md`.

---

## Deutsch (Kurzfassung)

Issues und Pull Requests sind ausdrücklich erwünscht – Fehlerberichte, Ideen, Fragen, Korrekturen, Übersetzungen, Doku. Ich antworte in der Regel **innerhalb von 3 Tagen**.

- **Fehler melden:** Was ist passiert, Gerät + iOS-Version, Windows-Version + Grafikkarte, Modus, und die Logdatei `%LOCALAPPDATA%\airplay\logs\airplay.log`.
- **Pull Requests:** Branch von `main`, `scripts/build.sh` (Build + Clippy) und `cargo test` müssen durchlaufen, Texte bitte auf Deutsch und Englisch (`src/app/strings.rs`). Größere Änderungen vorher kurz in einem Issue absprechen.
- Beiträge stehen unter der Lizenz des Projekts (GPL-3.0).
