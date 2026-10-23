"""Independent slow recursive scope/type oracle. No Hirsel or arena implementation.
Resolver declaration phases follow Crafting Interpreters 4a840f70 (MIT).
Dependency order comes from Python stdlib graphlib, with explicit declarations.
"""
import graphlib
import re

TOKEN = re.compile(r'//[^\n]*|\s+|[A-Za-z_][A-Za-z_0-9]*|[0-9]+|==|!=|<=|>=|.', re.ASCII)
POWER = {'==': 1, '!=': 1, '<': 2, '>': 2, '<=': 2, '>=': 2, '+': 3, '-': 3, '*': 4, '/': 4}

class Checker:
    def __init__(self, source, module, imports):
        self.tokens = [(m.group(), m.start(), m.end()) for m in TOKEN.finditer(source)
                       if not m.group().isspace() and not m.group().startswith('//')]
        self.tokens.append(('', len(source), len(source)))
        self.at, self.module, self.diagnostics = 0, module, []
        self.scopes, self.imports = [{}], imports

    def peek(self):
        return self.tokens[self.at][0]

    def pop(self):
        token = self.tokens[self.at]
        self.at += 1
        return token

    def emit(self, code, token):
        self.diagnostics.append(dict(module=self.module, code=code, start=token[1], end=token[2]))

    def expect(self, text):
        if self.peek() == text:
            self.pop()
            return True
        self.emit('syntax', self.tokens[self.at])
        return False

    def expression(self, minimum=1):
        token = self.pop()
        text = token[0]
        if text == '(':
            left = self.expression()
            self.expect(')')
        elif text.isdigit():
            left = 'int'
        elif text in ('true', 'false'):
            left = 'bool'
        elif re.fullmatch('[A-Za-z_][A-Za-z_0-9]*', text):
            if self.peek() == '.':
                self.pop()
                member = self.pop()
                text += '.' + member[0]
                token = (text, token[1], member[2])
            found = next((s[text] for s in reversed(self.scopes) if text in s), self.imports.get(text, 'missing'))
            if found in ('declared', 'missing'):
                self.emit('self_initializer' if found == 'declared' else 'undefined', token)
                left = 'unknown'
            else:
                left = found
        else:
            self.at -= 1
            self.emit('syntax', token)
            raise ValueError('expression')
        while POWER.get(self.peek(), 0) >= minimum:
            op = self.pop()
            right = self.expression(POWER[op[0]] + 1)
            if 'unknown' in (left, right):
                left = 'unknown'
            elif left != right or (op[0] not in ('==', '!=') and left != 'int'):
                self.emit('type_mismatch', op)
                left = 'unknown'
            else:
                left = 'int' if op[0] in ('+', '-', '*', '/') else 'bool'
        return left

    def run(self):
        while self.peek():
            token = self.pop()
            if token[0] == '{':
                self.scopes.append({})
            elif token[0] == '}':
                if len(self.scopes) == 1:
                    self.emit('syntax', token)
                else:
                    self.scopes.pop()
            elif token[0] == 'import':
                self.pop()
                self.expect(';')
            elif token[0] == 'let':
                name = self.pop()
                if not re.fullmatch('[A-Za-z_][A-Za-z_0-9]*', name[0]):
                    self.emit('syntax', name)
                    while self.peek() not in (';', '', 'let', '}'):
                        self.pop()
                    if self.peek() == ';': self.pop()
                    continue
                if not self.expect('='):
                    continue
                if name[0] in self.scopes[-1]: self.emit('duplicate', name)
                self.scopes[-1][name[0]] = 'declared'
                try:
                    kind = self.expression()
                except ValueError:
                    kind = 'unknown'
                self.scopes[-1][name[0]] = kind
                self.expect(';')
            else:
                self.emit('syntax', token)
        return self.scopes[0], sorted(self.diagnostics, key=lambda d: (d['start'], d['end'], d['code']))

def check(modules):
    graph = {name: set(re.findall(r'\bimport\s+(\w+)\s*;', source)) for name, source in modules.items()}
    sorter = graphlib.TopologicalSorter(graph)
    try:
        sorter.prepare()
    except graphlib.CycleError:
        pass
    summaries, diagnostics = {}, {}
    while sorter.is_active():
        ready = sorter.get_ready()
        for name in ready:
            if name not in modules:
                summaries[name] = {}
                continue
            imports = {dep + '.' + key: kind for dep in graph[name] for key, kind in summaries[dep].items()}
            summaries[name], diagnostics[name] = Checker(modules[name], name, imports).run()
        sorter.done(*ready)
    for name in modules:
        if name not in summaries:
            def reaches(a, b, seen=None):
                seen = set() if seen is None else seen
                if a in seen: return False
                seen.add(a)
                return b in graph[a] or any(reaches(x, b, seen) for x in graph[a] if x in graph)
            rows = []
            for m in re.finditer(r'\bimport\s+(\w+)\s*;', modules[name]):
                dep = m[1]
                code = 'import_cycle' if reaches(dep, name) else 'dependency_blocked'
                rows.append(dict(module=name, code=code, start=m.start(1), end=m.end(1)))
            diagnostics[name] = rows
    rows = [d for name in modules for d in diagnostics[name]]
    return dict(diagnostics=rows[:128], total=len(rows)), summaries

if __name__ == '__main__':
    import json, sys
    print(json.dumps(check(json.load(sys.stdin))[0], sort_keys=True))
