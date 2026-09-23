#!/usr/bin/env bash
# Python's own HTTPS client through a sealed run, on any OS (Git Bash on
# Windows): the server gets the value, the command holds the placeholder. On
# Windows there is no system bundle file, so this checks the bundle penv writes
# from the roots it carries.
source "$(dirname "$0")/common.sh"
REAL=sk_test_PY_REAL_0123456789_abcdefghijklmnop
cat > srv.py <<'PYEOF'
import http.server, ssl
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        open("seen.log", "a").write(str(self.headers.get("Authorization")) + "\n")
        body = b"ok"; self.send_response(200); self.send_header("Content-Length", "2"); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
s = http.server.HTTPServer(("127.0.0.1", 8546), H)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain("srv.pem", "srv.key")
s.socket = ctx.wrap_socket(s.socket, server_side=True)
s.serve_forever()
PYEOF
"$PY" srv.py &
wait_port 8546
printf '# @type=string(startsWith=sk_test_, minLength=40) @hosts=localhost\nSTRIPE_SECRET_KEY=\n' > .env.schema
printf 'STRIPE_SECRET_KEY=%s\n' "$REAL" > .env
cat > client.py <<'PYEOF'
import os, urllib.request
key = os.environ["STRIPE_SECRET_KEY"]
req = urllib.request.Request("https://localhost:8546/", headers={"Authorization": "Bearer " + key})
print("status", urllib.request.urlopen(req).status, "placeholder", key != "")
PYEOF
out=$(SSL_CERT_FILE="$(native "$WORK/trust.pem")" "$PENV" run --sealed -- "$PY" client.py 2>&1) || fail "$out"
echo "$out" | grep -q "status 200" || fail "$out"
grep -q "Bearer $REAL" seen.log || fail "the server did not get the value: $(cat seen.log)"
echo "$out" | grep -q "$REAL" && fail "the value reached the command's output"
echo "ok sealed python"
