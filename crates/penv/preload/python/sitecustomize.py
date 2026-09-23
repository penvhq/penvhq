# Written by penv run and put first on PYTHONPATH. It masks this run's sensitive
# values where the output pipe never sees them: log records, which in-process
# shippers such as Sentry read, and bytes the app sends on connections it
# accepted. Connections the app opens itself are left alone: a secret sent to its
# own API is doing its job. Key names arrive in PENV_SENSITIVE; values come from
# os.environ. Any failure leaves the app as it was. A project's own
# sitecustomize still runs afterwards.
def _penv_preload():
    import base64
    import binascii
    import json
    import os
    import sys
    from urllib.parse import quote

    here = os.path.dirname(os.path.abspath(__file__))
    mask = "\u2592" * 6
    forms = []
    for name in filter(None, os.environ.get("PENV_SENSITIVE", "").split(",")):
        value = os.environ.get(name)
        if not value or len(value) < 4:
            continue
        raw = value.encode("utf-8")
        b64 = base64.b64encode(raw).decode("ascii")
        for form in (value, quote(value, safe=""), b64, b64.rstrip("="),
                     binascii.hexlify(raw).decode("ascii"), json.dumps(value)[1:-1]):
            if len(form) >= 4 and form not in forms:
                forms.append(form)
    forms.sort(key=len, reverse=True)

    def mask_text(text):
        for form in forms:
            if form in text:
                text = text.replace(form, form[:2] + mask)
        return text

    # Served bytes keep their length, so a Content-Length the app already sent
    # still matches: two bytes kept, the rest replaced with "*".
    byte_forms = []
    for f in forms:
        raw = f.encode("utf-8")
        byte_forms.append((raw, raw[:2] + b"*" * (len(raw) - 2)))

    def mask_bytes(data):
        try:
            data = bytes(data)
        except Exception:
            return data
        for form, masked in byte_forms:
            if form in data:
                data = data.replace(form, masked)
        return data

    if forms:
        import logging
        get_message = logging.LogRecord.getMessage

        def penv_get_message(self):
            message = get_message(self)
            try:
                return mask_text(message)
            except Exception:
                return message

        logging.LogRecord.getMessage = penv_get_message

        # Handlers that read record.msg and record.args themselves (Sentry's
        # logging integration, structured shippers) see masked text too.
        init = logging.LogRecord.__init__

        def penv_init(self, *args, **kwargs):
            init(self, *args, **kwargs)
            try:
                if isinstance(self.msg, str):
                    self.msg = mask_text(self.msg)
                if isinstance(self.args, tuple):
                    self.args = tuple(mask_text(a) if isinstance(a, str) else a for a in self.args)
                elif isinstance(self.args, dict):
                    self.args = {k: mask_text(v) if isinstance(v, str) else v for k, v in self.args.items()}
            except Exception:
                pass

        logging.LogRecord.__init__ = penv_init

        import socket

        class PenvServed(socket.socket):
            """A connection the app accepted: what it sends goes to a client."""

            def send(self, data, *args):
                return super().send(mask_bytes(data), *args)

            def sendall(self, data, *args):
                return super().sendall(mask_bytes(data), *args)

        accept = socket.socket.accept

        def penv_accept(self):
            conn, address = accept(self)
            try:
                served = PenvServed(conn.family, conn.type, conn.proto, fileno=conn.detach())
                served.settimeout(self.gettimeout())
                return served, address
            except Exception:
                return conn, address

        socket.socket.accept = penv_accept

    # Hand over to the project's own sitecustomize, if it has one.
    sys.path[:] = [p for p in sys.path if os.path.abspath(p or ".") != here]
    sys.modules.pop("sitecustomize", None)
    try:
        import sitecustomize  # noqa: F401
    except ImportError:
        pass
    finally:
        sys.path.insert(0, here)


try:
    _penv_preload()
except Exception:
    pass
