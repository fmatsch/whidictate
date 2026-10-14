# whidictate-server betreiben

## Mit Docker (empfohlen)

1. `deploy/Caddyfile`: Servernamen eintragen (z. B. `diktat.kanzlei.local`).
2. `deploy/docker-compose.yml`: Startpasswort für den Benutzer `admin` setzen.
3. `docker compose -f deploy/docker-compose.yml up -d --build`
4. `https://<servername>/` öffnen, als `admin` anmelden, Passwort ändern („Passwort“ oben rechts),
   in „Benutzer“ die Anwälte (Rolle *Autor*) und das Sekretariat (Rolle *Sekretariat*) anlegen.
5. Die Zeile `WHIDICTATE_ADMIN_PASSWORD` danach aus der Compose-Datei entfernen.

Bei `tls internal` erzeugt Caddy ein eigenes Zertifikat. Das Stammzertifikat
(`caddy-data` → `pki/authorities/local/root.crt`) auf den Arbeitsplätzen als vertrauenswürdig installieren,
sonst verweigert der Client die Verbindung.

## Ohne Docker

```bash
whidictate-server --data-dir /var/lib/whidictate create-user admin --role admin
whidictate-server --data-dir /var/lib/whidictate serve --listen 127.0.0.1:8484 --secure-cookies
```

Einen Reverse-Proxy mit TLS davor setzen (Caddy, nginx). Dabei `client_max_body_size` / `max_size`
mindestens 300 MB erlauben (lange Diktate als WAV).

## Rollen

| Rolle | Darf |
|---|---|
| Autor | eigene Diktate senden, Status und fertigen Text lesen, Audio anhören |
| Sekretariat | alle Diktate sehen, bearbeiten, Status ändern, zuweisen |
| Administrator | wie Sekretariat, dazu Benutzer verwalten und Diktate löschen |

## Datensicherung

Alles liegt im Datenverzeichnis: `whidictate.db` (SQLite) und `audio/`. Zum Sichern den Server kurz
stoppen oder `sqlite3 whidictate.db ".backup backup.db"` verwenden und `audio/` mitkopieren.

## Sicherheit

- Anmeldung: Argon2id-Passwörter, 5 Fehlversuche → 60 s Sperre pro Benutzername
- Browser-Sitzung 12 h (HttpOnly/SameSite=Strict-Cookie), Desktop-Client-Token 90 Tage; Sperren eines Benutzers
  beendet alle seine Sitzungen sofort
- Daten **nicht** auf Anwendungsebene verschlüsselt → Festplattenverschlüsselung des Servers verwenden
- Kein Zugriff aus dem Internet freigeben; VPN oder internes Netz
