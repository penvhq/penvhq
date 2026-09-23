#!/usr/bin/env bash
# A WebSocket through a sealed run: the handshake carries the value, messages
# from the server come back with it swapped, fragmented or not, and a frame
# using an extension nobody agreed to ends the connection. Needs python3 and
# Node 22.21 or later.
source "$(dirname "$0")/common.sh"
REAL=sk_test_E2E_REAL_0123456789_abcdefghijkl
cat > ws.py <<'PY'
import socket, ssl, threading, base64, hashlib, struct
def handle(c):
    data = b""
    while b"\r\n\r\n" not in data:
        data += c.recv(4096)
    head = data.split(b"\r\n\r\n")[0].decode()
    line = head.split("\r\n")[0]
    h = {l.split(":", 1)[0].lower(): l.split(":", 1)[1].strip() for l in head.split("\r\n")[1:]}
    open("seen.log", "a").write(line + " ext=" + h.get("sec-websocket-extensions", "none") + "\n")
    acc = base64.b64encode(hashlib.sha1((h["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
    c.sendall(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: %s\r\n\r\n" % acc).encode())
    def exact(n):
        b = b""
        while len(b) < n:
            chunk = c.recv(n - len(b))
            if not chunk: raise EOFError
            b += chunk
        return b
    def frame(first, body):
        return bytes([first]) + (bytes([len(body)]) if len(body) < 126 else bytes([126]) + struct.pack(">H", len(body))) + body
    while True:
        try: x = exact(2)
        except EOFError: return
        op, ln = x[0] & 15, x[1] & 127
        if ln == 126: ln = struct.unpack(">H", exact(2))[0]
        mask = exact(4); p = bytes(b ^ mask[i % 4] for i, b in enumerate(exact(ln)))
        if op == 8: c.sendall(b"\x88\x00"); return
        open("seen.log", "a").write("message " + p.decode() + "\n")
        reply = ("echo " + line).encode()
        if p == b"frag":
            cut = reply.index(b"key=") + 12
            c.sendall(frame(0x01, reply[:cut]) + frame(0x89, b"") + frame(0x80, reply[cut:]))
        elif p == b"rsv":
            c.sendall(frame(0xC1, reply))
        else:
            c.sendall(frame(0x81, reply))
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1", 8545)); s.listen(8)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain("srv.pem", "srv.key")
while True:
    c, _ = s.accept()
    try: c = ctx.wrap_socket(c, server_side=True)
    except Exception: continue
    threading.Thread(target=handle, args=(c,), daemon=True).start()
PY
python3 ws.py &
wait_port 8545
printf '# @type=string(startsWith=sk_test_, minLength=40) @hosts=localhost\nSTRIPE_SECRET_KEY=\n' > .env.schema
printf 'STRIPE_SECRET_KEY=%s\n' "$REAL" > .env
cat > client.js <<'JS'
const key = process.env.STRIPE_SECRET_KEY;
const ws = new WebSocket("wss://localhost:8545/s?key=" + key, { headers: { "Sec-WebSocket-Extensions": "permessage-deflate" } });
ws.onopen = () => ws.send(process.argv[2]);
ws.onmessage = (m) => { process.stdout.write(m.data.includes("E2E_REAL") ? "LEAK " : m.data.includes(key) ? "swapped " : "other "); ws.close(); };
ws.onclose = () => process.stdout.write("closed\n");
ws.onerror = () => process.stdout.write("error ");
JS
for mode in plain frag rsv; do
  out=$(SSL_CERT_FILE="$(native "$WORK/trust.pem")" "$PENV" run --sealed --no-mask -- node client.js "$mode" 2>/dev/null) || fail "$mode: $out"
  case "$mode:$out" in
    plain:*"swapped closed"*|frag:*"swapped closed"*) ;;
    rsv:*LEAK*|rsv:*swapped*) fail "rsv delivered a frame: $out" ;;
    rsv:*closed*) ;;
    *) fail "$mode: $out" ;;
  esac
done
grep -q "key=$REAL" seen.log || fail "the handshake did not carry the value: $(cat seen.log)"
grep -q "ext=none" seen.log || fail "compression was offered upstream"
grep -q "message .*$REAL" seen.log && fail "a message from the command carried the value"
echo "ok sealed websocket"
