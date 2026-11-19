#!/usr/bin/env python3
"""Rewraps the prose of markdown docs at 78 columns, in place.

Paragraphs and list items are reflowed; tables, fenced blocks, headings,
html and block quotes are left as they are, an inline code span is never
split, and no line is made to open with a list, heading or table marker.
Fails before writing a doc whose text would change beyond whitespace.

    tools/reflow_md.py docs/research/<doc>.md...

Reflow a doc when no open branch edits it, one commit per doc: the whole
doc shows as rewritten in a diff.
"""
import re, sys
W = 78
LIST = re.compile(r'^(\s*)([-*+]|\d+\.)(\s+)(.*)$')
def special(l):
    s = l.lstrip()
    return (not s) or s.startswith(('|', '#', '```', '<', '>')) or re.match(r'^(-{3,}|\*{3,})$', s)
def tokens(text):
    out, cur, ticks = [], [], 0
    for w in text.split():
        cur.append(w); ticks += w.count('`')
        if ticks % 2 == 0:
            out.append(' '.join(cur)); cur = []; ticks = 0
    if cur: out.extend(cur)
    return out
def wrap(text, first, rest):
    lines, line = [], first
    pre = first
    for t in tokens(text):
        cand = t if line == pre else line + ' ' + t
        bad = re.match(r'^([-*+>|#]|\d+[.)]$|```)', t)
        if line != pre and len(cand) > W and not bad:
            lines.append(line); pre = rest; line = rest + t
        else:
            line = cand if line != pre else pre + t
    lines.append(line)
    return lines
def reflow(src):
    L = src.split('\n'); out = []; i = 0; fence = False
    while i < len(L):
        l = L[i]
        if l.lstrip().startswith('```'):
            fence = not fence; out.append(l); i += 1; continue
        if fence or special(l):
            out.append(l); i += 1; continue
        m = LIST.match(l)
        if m:
            ind, mk, sp, body = m.groups()
            first = ind + mk + ' '; rest = ' ' * len(first); parts = [body]; i += 1
            while i < len(L) and not special(L[i]) and not LIST.match(L[i]) and not L[i].lstrip().startswith('```') \
                  and len(L[i]) - len(L[i].lstrip()) > len(ind):
                parts.append(L[i].strip()); i += 1
            out += wrap(' '.join(parts), first, rest); continue
        ind = l[:len(l) - len(l.lstrip())]; parts = [l.strip()]; i += 1
        while i < len(L) and not special(L[i]) and not LIST.match(L[i]) and not L[i].lstrip().startswith('```') \
              and L[i][:len(ind)] == ind and (len(L[i]) - len(L[i].lstrip())) == len(ind):
            parts.append(L[i].strip()); i += 1
        out += wrap(' '.join(parts), ind, ind)
    return '\n'.join(out)
for p in sys.argv[1:]:
    s = open(p, encoding='utf-8').read(); t = reflow(s)
    assert re.sub(r'\s+', ' ', s) == re.sub(r'\s+', ' ', t), p
    open(p, 'w', encoding='utf-8').write(t)
