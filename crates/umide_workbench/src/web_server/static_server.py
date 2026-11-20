# Embedded by Umide and executed with Python's isolated mode in the project root.
import http.server
import os
import sys

root = os.path.realpath(os.getcwd())


def is_public_file(path):
    resolved = os.path.realpath(path)
    try:
        if os.path.commonpath((root, resolved)) != root:
            return False
    except ValueError:  # A link can point to another drive on Windows.
        return False
    parts = os.path.relpath(resolved, root).split(os.sep)
    return all(p in (".", ".well-known") or not p.startswith(".") for p in parts)


class ProjectFiles(http.server.SimpleHTTPRequestHandler):
    def send_head(self):
        path = self.translate_path(self.path)
        if not is_public_file(path):
            self.send_error(404)
            return None
        # The base handler selects a directory index after translate_path.
        if os.path.isdir(path):
            for name in ("index.html", "index.htm"):
                index = os.path.join(path, name)
                if os.path.isfile(index):
                    if not is_public_file(index):
                        self.send_error(404)
                        return None
                    break
        return super().send_head()

    def list_directory(self, path):
        self.send_error(404)
        return None


http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), ProjectFiles).serve_forever()
