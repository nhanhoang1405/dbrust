"""Small VT screen recorder for terminal smoke-test assertions (stdlib only)."""
import codecs
import re
import unicodedata


class Screen:
    def __init__(self, width=120, height=40):
        self.width, self.height = width, height
        self.cells = [[" "] * width for _ in range(height)]
        self.x = self.y = 0
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.pending = ""

    @property
    def text(self):
        return "\n".join("".join(row) for row in self.cells)

    def feed(self, chunk):
        self.pending += self.decoder.decode(chunk)
        while self.pending:
            if self.pending.startswith("\x1b["):
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", self.pending)
                if not match:
                    return
                args, _, command = match.groups()
                values = [int(n) if n.isdigit() else 0 for n in args.split(";")]
                n = values[0] or 1
                if command in "Hf":
                    self.y = min(self.height - 1, n - 1)
                    self.x = min(self.width - 1, (values[1] or 1) - 1 if len(values) > 1 else 0)
                elif command == "G":
                    self.x = min(self.width - 1, n - 1)
                elif command == "d":
                    self.y = min(self.height - 1, n - 1)
                elif command == "A":
                    self.y = max(0, self.y - n)
                elif command == "B":
                    self.y = min(self.height - 1, self.y + n)
                elif command == "C":
                    self.x = min(self.width - 1, self.x + n)
                elif command == "D":
                    self.x = max(0, self.x - n)
                elif command == "J" and values[0] in (2, 3):
                    self.cells = [[" "] * self.width for _ in range(self.height)]
                elif command == "K":
                    begin, end = (0, self.width) if values[0] == 2 else ((0, self.x + 1) if values[0] == 1 else (self.x, self.width))
                    self.cells[self.y][begin:end] = [" "] * (end - begin)
                self.pending = self.pending[match.end():]
                continue
            if self.pending.startswith("\x1b"):
                if len(self.pending) < 2:
                    return
                self.pending = self.pending[2:]
                continue
            char, self.pending = self.pending[0], self.pending[1:]
            if char == "\r":
                self.x = 0
            elif char == "\n":
                self.y = min(self.height - 1, self.y + 1)
            elif char == "\b":
                self.x = max(0, self.x - 1)
            elif char >= " " and not unicodedata.combining(char):
                if self.x < self.width:
                    self.cells[self.y][self.x] = char
                self.x += 2 if unicodedata.east_asian_width(char) in ("W", "F") else 1
