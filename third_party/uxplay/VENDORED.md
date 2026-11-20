# UxPlay-Kern (vendored)

- Quelle: https://github.com/FDH2/UxPlay
- Tag: v1.73.7 (2026-09-04), Commit df67c212a433cf6dda3676dd40c097900d24e645
- Übernommen am 2026-11-19: nur `lib/` und `LICENSE` (GPL-3.0)
- Änderungen am Kern: keine.
- `lib/dnssd.c` wird nicht gebaut. Ersatz mit derselben API (`lib/dnssd.h`):
  `src/discovery/dnssd_win.cpp` (Ankündigung über Windows DNS-SD statt Bonjour).
- Build-Defines wie `lib/CMakeLists.txt` unter Windows, inkl. `NOHOLD`
  (neues Gerät übernimmt die Verbindung); `-march=native` bewusst nicht gesetzt.
