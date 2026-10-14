#!/usr/bin/env bash
# Startet einen Server mit leerem Datenverzeichnis und prüft den Ablauf Autor -> Sekretariat.
set -euo pipefail
BIN="${BIN:-target/debug/whidictate-server}"
DATA="$(mktemp -d)"
PORT="${PORT:-18484}"
URL="http://127.0.0.1:$PORT"
trap 'kill $PID 2>/dev/null || true; rm -rf "$DATA"' EXIT

WHIDICTATE_PASSWORD=autorpasswort1 "$BIN" --data-dir "$DATA" create-user anna --display-name "Anna Anwalt" --role author >/dev/null
WHIDICTATE_PASSWORD=sekretpasswort1 "$BIN" --data-dir "$DATA" create-user sabine --display-name "Sabine Sekretariat" --role secretary >/dev/null
"$BIN" --data-dir "$DATA" serve --listen "127.0.0.1:$PORT" >"$DATA/server.log" 2>&1 &
PID=$!
for _ in $(seq 50); do curl -sf "$URL/api/health" >/dev/null && break; sleep 0.1; done

ok()   { echo "  ok: $1"; }
fail() { echo "FEHLER: $1"; cat "$DATA/server.log"; exit 1; }
j()    { python3 -c "import sys,json; d=json.load(sys.stdin); print($1)"; }

echo "Anmeldung"
curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/api/login" -H 'content-type: application/json' -d '{"username":"anna","password":"falsch"}' | grep -q 401 || fail "falsches Passwort muss 401 liefern"; ok "falsches Passwort abgelehnt"
A=$(curl -sf -X POST "$URL/api/login" -H 'content-type: application/json' -d '{"username":"anna","password":"autorpasswort1"}' | j "d['token']")
S=$(curl -sf -X POST "$URL/api/login" -H 'content-type: application/json' -d '{"username":"sabine","password":"sekretpasswort1"}' | j "d['token']")
ok "Autor und Sekretariat angemeldet"

echo "Upload"
head -c 20000 /dev/urandom > "$DATA/a.wav"
META='{"client_ref":"ref-1","matter":"12 O 345/26","instruction":"Bitte als Schriftsatz, eilig","priority":1,"language":"de","duration_s":42.5,"text":"Sehr geehrter Herr Müller,\nich danke Ihnen.","raw_text":"Sehr geehrter Herr Müller, neue Zeile ich danke Ihnen.","segments":[{"start":0.0,"end":2.5,"text":"Sehr geehrter Herr Müller"},{"start":2.5,"end":5.0,"text":"ich danke Ihnen"}]}'
R1=$(curl -sf -X POST "$URL/api/dictations" -H "Authorization: Bearer $A" -F "meta=$META;type=application/json" -F "audio=@$DATA/a.wav;type=audio/wav")
ID=$(echo "$R1" | j "d['id']")
R2=$(curl -sf -X POST "$URL/api/dictations" -H "Authorization: Bearer $A" -F "meta=$META;type=application/json" -F "audio=@$DATA/a.wav;type=audio/wav")
[ "$(echo "$R2" | j "d['id']")" = "$ID" ] || fail "Upload muss idempotent sein"; ok "doppelter Upload erzeugt kein zweites Diktat"

echo "Rechte"
[ "$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/dictations")" = 401 ] || fail "ohne Anmeldung 401"; ok "ohne Anmeldung gesperrt"
[ "$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$URL/api/dictations/$ID/text" -H "Authorization: Bearer $A" -H 'content-type: application/json' -d '{"text":"x","base_rev":1}')" = 403 ] || fail "Autor darf Text nicht bearbeiten"; ok "Autor darf nicht bearbeiten"
[ "$(curl -sf "$URL/api/dictations" -H "Authorization: Bearer $S" | j "len(d)")" = 1 ] || fail "Sekretariat sieht Diktat"; ok "Sekretariat sieht das Diktat"

echo "Bearbeitung"
[ "$(curl -sf "$URL/api/dictations/$ID" -H "Authorization: Bearer $S" | j "d['segments'][1]['text']")" = "ich danke Ihnen" ] || fail "Segmente"; ok "Segmente gespeichert"
curl -sf -X PUT "$URL/api/dictations/$ID/text" -H "Authorization: Bearer $S" -H 'content-type: application/json' -d '{"text":"Sehr geehrter Herr Dr. Müller,\nich danke Ihnen.","base_rev":1}' >/dev/null; ok "Text bearbeitet (rev 2)"
[ "$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$URL/api/dictations/$ID/text" -H "Authorization: Bearer $S" -H 'content-type: application/json' -d '{"text":"alt","base_rev":1}')" = 409 ] || fail "veraltete Version muss 409 liefern"; ok "Bearbeitungskonflikt erkannt"
[ "$(curl -sf "$URL/api/dictations/$ID" -H "Authorization: Bearer $S" | j "d['status']")" = in_progress ] || fail "Status in_progress nach Bearbeitung"; ok "Status automatisch 'in Bearbeitung'"
curl -sf -X POST "$URL/api/dictations/$ID/status" -H "Authorization: Bearer $S" -H 'content-type: application/json' -d '{"status":"done","note":"fertig geschrieben"}' >/dev/null; ok "als fertig markiert"

echo "Autor sieht Ergebnis"
[ "$(curl -sf "$URL/api/dictations?mine=1" -H "Authorization: Bearer $A" | j "d[0]['status']")" = done ] || fail "Autor sieht Status"; ok "Autor sieht Status 'fertig'"
[ "$(curl -sf "$URL/api/dictations/$ID" -H "Authorization: Bearer $A" | j "d['text'].startswith('Sehr geehrter Herr Dr.')")" = True ] || fail "Autor sieht bearbeiteten Text"; ok "Autor sieht bearbeiteten Text"

echo "Audio mit Range"
[ "$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/dictations/$ID/audio" -H "Authorization: Bearer $A" -H 'Range: bytes=100-199')" = 206 ] || fail "Range"; ok "Audio-Teilabruf (206)"
[ "$(curl -s "$URL/api/dictations/$ID/audio" -H "Authorization: Bearer $S" | wc -c | tr -d ' ')" = 20000 ] || fail "Audio komplett"; ok "Audio vollständig"

echo "Verlauf"
[ "$(curl -sf "$URL/api/dictations/$ID/history" -H "Authorization: Bearer $S" | j "len(d['revisions'])")" = 2 ] || fail "2 Versionen"; ok "2 Versionen im Verlauf"
echo "ALLES OK"
