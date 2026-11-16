import re, html
md = open(__import__('sys').argv[1]).read()
page = __import__('os').environ.get('BENCH_DIR', '.') + '/audio-limits.html'
def inline(t):
    t = html.escape(t, quote=False)
    t = re.sub(r'\*\*(.+?)\*\*', r'<b>\1</b>', t)
    t = re.sub(r'`(.+?)`', r'<code>\1</code>', t)
    return t
def pill(v):
    raw = re.sub(r'\*\*|`', '', v).strip().lower()
    cls = 'ok' if raw.startswith(('yes', 'fixed')) else 'no' if raw.startswith(('no', 'worse')) else 'maybe'
    return f'<span class="pill {cls}">{inline(v)}</span>'
def table(title):
    sec = md.split('## ' + title, 1)[1].split('\n## ', 1)[0]
    rows = [l for l in sec.splitlines() if l.startswith('| ') and not l.startswith('|---')]
    head = [c.strip() for c in rows[0].strip('|').split('|')]
    out = ['<div class="tablewrap"><table><thead><tr>' + ''.join(f'<th>{h}</th>' for h in head if h != 'Date') + '</tr></thead><tbody>']
    for r in rows[1:]:
        c = [x.strip() for x in r.strip().strip('|').split('|')]
        c = [x for i, x in enumerate(c) if head[i] != 'Date']
        cells = [f'<td class="num">{inline(c[0])}</td>'] + [f'<td>{inline(x)}</td>' for x in c[1:-1]] + [f'<td>{pill(c[-1])}</td>']
        out.append('<tr>' + ''.join(cells) + '</tr>')
    out.append('</tbody></table></div>')
    return '\n'.join(out)
notes = md.split('## Method findings', 1)[1].split('\n', 1)[1]
items = ''.join(f'<li>{inline(l[2:])}</li>' for l in notes.strip().splitlines() if l.startswith('- '))
block = f'''  <section id="testlog">
    <h2>Test log: everything tried so far</h2>
    <p>So nothing is run twice. Verdicts: yes (helps), no (no useful change), worse, superseded, open. The same log is in docs/research/bk4829-bench-log.md.</p>
    <h3 class="charthead">Transmit</h3>
{table("Transmit (radio sends, bladeRF captures; `txtest`)")}
    <h3 class="charthead">Receive</h3>
{table("Receive (bladeRF sends FM, the radio's audio to the PC)")}
    <h3 class="charthead">Method findings</h3>
    <ul>{items}</ul>
  </section>

'''
s = open(page).read()
s = re.sub(r'  <section id="testlog">.*?</section>\n\n', '', s, flags=re.S)
anchor = '  <section>\n    <h2>How this was measured, and its limits</h2>'
assert s.count(anchor) == 1
s = s.replace(anchor, block + anchor)
open(page, 'w').write(s)
print('ok')
