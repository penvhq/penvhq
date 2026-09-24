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

        # A traceback a handler formats (record.exc_text, stack_info) carries
        # the exception's text, which getMessage never sees.
        format_exception = logging.Formatter.formatException
        format_stack = logging.Formatter.formatStack

        def penv_format_exception(self, ei):
            text = format_exception(self, ei)
            try:
                return mask_text(text)
            except Exception:
                return text

        def penv_format_stack(self, stack_info):
            text = format_stack(self, stack_info)
            try:
                return mask_text(text)
            except Exception:
                return text

        logging.Formatter.formatException = penv_format_exception
        logging.Formatter.formatStack = penv_format_stack

        import socket

        class PenvServed(socket.socket):
            """A connection the app accepted: what it sends goes to a client.
            After a partial send the caller resends its own unmasked rest; the
            masked rest goes instead, so a value the cut split stays masked."""

            def _penv_masked(self, data):
                data = bytes(data)
                rest = getattr(self, "_penv_rest", None)
                if rest is not None and rest[0] == data:
                    return data, rest[1]
                return data, mask_bytes(data)

            def _penv_sent(self, data, masked, sent):
                self._penv_rest = (data[sent:], masked[sent:]) if 0 <= sent < len(data) else None
                return sent

            def send(self, data, *args):
                data, masked = self._penv_masked(data)
                return self._penv_sent(data, masked, super().send(masked, *args))

            def sendall(self, data, *args):
                self._penv_rest = None
                return super().sendall(mask_bytes(data), *args)

            def sendto(self, data, *args):
                data, masked = self._penv_masked(data)
                return self._penv_sent(data, masked, super().sendto(masked, *args))

            def sendmsg(self, buffers, *args):
                data, masked = self._penv_masked(b"".join(bytes(b) for b in buffers))
                return self._penv_sent(data, masked, super().sendmsg([masked], *args))

        accept = socket.socket.accept

        def penv_accept(self):
            conn, address = accept(self)
            try:
                # The accepted connection's own timeout, which CPython sets from
                # the default, not from the listening socket.
                timeout = conn.gettimeout()
                served = PenvServed(conn.family, conn.type, conn.proto, fileno=conn.detach())
                served.settimeout(timeout)
                return served, address
            except Exception:
                return conn, address

        socket.socket.accept = penv_accept

    # Hand over to the project's own sitecustomize, if it has one. The import
    # machinery takes this module back out of sys.modules once it finishes, so
    # with no other one this module goes back in.
    sys.path[:] = [p for p in sys.path if os.path.abspath(p or ".") != here]
    me = sys.modules.pop("sitecustomize", None)
    try:
        import sitecustomize  # noqa: F401
    except ImportError:
        pass
    finally:
        sys.path.insert(0, here)
        if me is not None and "sitecustomize" not in sys.modules:
            sys.modules["sitecustomize"] = me


try:
    _penv_preload()
except Exception:
    pass
