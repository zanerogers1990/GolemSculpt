"""Keeps the tooltip translations in crates/toolkit/translations/ in step with
the English in the source.

The app looks a tooltip up by its English text (crates/toolkit/src/translate.rs),
so a language file is a list of English sentences, each with its translation.
This script finds those sentences: every string literal in the app and tool
crates that reads like one (it starts with a capital, has a space, ends in
. ? ! or a bracket), outside tests, scripted demos and the Claude bridge. It
takes in more than tooltips (status messages too); that costs a little
translating and nothing at run time.

    python tools/translations.py status
        How many sentences there are, and for each language how many are
        translated, missing, or no longer in the source.
    python tools/translations.py todo OUT_DIR [CHUNK]
        Writes the sentences some language is missing to OUT_DIR/chunk_NN.json,
        CHUNK to a file (default 200), as [{"id": n, "en": "..."}].
    python tools/translations.py check LANG RESULT.json...
        Checks translations written as {"id": "translation"} against the chunk
        files beside... (pass --chunks DIR): placeholders, line breaks, keys,
        on-screen names. Errors must be fixed; warnings are worth a look.
    python tools/translations.py merge LANG --chunks DIR RESULT.json... [--glossary FILE]
        Puts the checked translations into LANG's file, in source order.

A sentence built with format! is kept with its placeholders numbered, {} as
{0}, {1}... and {name:.1} as {name}; the translation keeps the same ones.
"""
import json
import os
import re
import sys
from collections import OrderedDict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FILES_DIR = os.path.join(ROOT, 'crates', 'toolkit', 'translations')

# The languages, as crates/toolkit/src/translate.rs lists them.
LANGUAGES = OrderedDict([
    ('es', 'Spanish'),
    ('pt', 'Portuguese (Brazil)'),
    ('fr', 'French'),
    ('de', 'German'),
    ('it', 'Italian'),
    ('pl', 'Polish'),
    ('tr', 'Turkish'),
    ('ru', 'Russian'),
    ('ja', 'Japanese'),
    ('zh', 'Chinese (Simplified)'),
    ('ko', 'Korean'),
])

# Text a person never reads in the app: Claude's bridge, scripted demos,
# benchmarks and the brush-feel fixtures.
SKIP_FILE = re.compile(r'(^|[\\/])(mcp|bridge|bridge_guard|brush_hash|brush_feel|bench|bench_render|ui_demo|demo|play|paint_play)\.rs$')
SKIP_DIR = re.compile(r'(^|[\\/])(bridge|ui_demo|play|tests?|target)([\\/]|$)')
# Sentence-like, but names: the picker's languages.
SKIP_TEXT = {'Português (Brasil)', 'Chinese (Simplified)'}


# --- reading Rust ---------------------------------------------------------

def literals(src):
    """Every string literal in `src` as (value, offset), and `src` with
    literals and comments blanked out (same length, newlines kept)."""
    i, n = 0, len(src)
    code = list(src)
    out = []

    def blank(a, b):
        for k in range(a, b):
            if code[k] != '\n':
                code[k] = ' '

    while i < n:
        c = src[i]
        if src.startswith('//', i):
            j = src.find('\n', i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
            continue
        if src.startswith('/*', i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith('/*', j):
                    depth, j = depth + 1, j + 2
                elif src.startswith('*/', j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        prev = src[i - 1] if i else ' '
        after_ident = prev.isalnum() or prev == '_'
        raw = None if after_ident else re.match(r'(b?)r(#*)"', src[i:i + 40])
        if raw:
            hashes = raw.group(2)
            start = i + len(raw.group(0))
            end = src.find('"' + hashes, start)
            if not raw.group(1):
                out.append((src[start:end], i))
            stop = end + 1 + len(hashes)
            blank(i, stop)
            i = stop
            continue
        if c == '"' or (c == 'b' and src.startswith('b"', i) and not after_ident):
            byte = c == 'b'
            j = i + (2 if byte else 1)
            val = []
            while src[j] != '"':
                if src[j] == '\\':
                    e = src[j + 1]
                    if e in 'ntr0':
                        val.append({'n': '\n', 't': '\t', 'r': '\r', '0': '\0'}[e])
                        j += 2
                    elif e in '\\"\'':
                        val.append(e)
                        j += 2
                    elif e == 'x':
                        val.append(chr(int(src[j + 2:j + 4], 16)))
                        j += 4
                    elif e == 'u':
                        k = src.index('}', j)
                        val.append(chr(int(src[j + 3:k], 16)))
                        j = k + 1
                    elif e in '\r\n':
                        j += 1
                        while src[j] in ' \t\r\n':
                            j += 1
                    else:
                        raise ValueError('unknown escape \\' + e)
                else:
                    val.append(src[j])
                    j += 1
            if not byte:
                out.append((''.join(val), i))
            blank(i, j + 1)
            i = j + 1
            continue
        if c == "'":
            # A char literal ('a', '\n', '\'', '\u{..}'), or a lifetime.
            if src[i + 1] == '\\':
                j = i + 3 if src[i + 2] == "'" else src.index("'", i + 3)
                blank(i, j + 1)
                i = j + 1
                continue
            if i + 2 < n and src[i + 2] == "'":
                blank(i, i + 3)
                i += 3
                continue
        i += 1
    return out, ''.join(code)


def test_modules(code):
    """The spans of `#[cfg(test)] mod x { ... }` blocks in blanked code."""
    spans = []
    for m in re.finditer(r'#\[cfg\(test\)\]\s*(pub(\([^)]*\))?\s+)?mod\s+\w+\s*\{', code):
        depth, j = 1, m.end()
        while depth:
            if code[j] == '{':
                depth += 1
            elif code[j] == '}':
                depth -= 1
            j += 1
        spans.append((m.start(), j))
    return spans


def source_files():
    for base in ('src', 'crates'):
        for d, dirs, files in os.walk(os.path.join(ROOT, base)):
            rel = os.path.relpath(d, ROOT)
            dirs[:] = sorted(x for x in dirs if not SKIP_DIR.search(os.path.join(rel, x)) and 'test' not in x)
            for f in sorted(files):
                path = os.path.join(rel, f)
                if f.endswith('.rs') and 'test' not in f and not SKIP_FILE.search(path):
                    yield path


PLACEHOLDER = re.compile(r'\{\{|\}\}|\{(\w*)(?::[^{}]*)?\}')


def normalise(text):
    """`text` as the app's table keeps it: trimmed, format placeholders
    numbered. Returns None for something that is not a sentence to translate."""
    text = text.strip()
    starts = text[:1].isupper() or re.match(r'\{\w*(:[^{}]*)?\}', text)
    if len(text) < 8 or ' ' not in text or not starts or not re.search('[a-z]', text):
        return None
    if text[-1] not in '.?!)':
        return None
    has_slot = False
    count = 0

    def slot(m):
        nonlocal has_slot, count
        if m.group(0) in ('{{', '}}'):
            return m.group(0)
        has_slot = True
        name = m.group(1)
        if name == '':
            name = str(count)
            count += 1
        return '{' + name + '}'

    text = PLACEHOLDER.sub(slot, text)
    if not has_slot:
        # A format string with nothing to fill in shows {{ as {.
        text = text.replace('{{', '{').replace('}}', '}')
    elif len(re.sub(r'\{\w*\}', '', text).strip()) < 6:
        return None  # nothing but placeholders: nothing to translate
    return text


def sentences():
    """English sentence -> where it first appears, in source order."""
    found = OrderedDict()
    for path in source_files():
        src = open(os.path.join(ROOT, path), encoding='utf-8').read()
        lits, code = literals(src)
        tests = test_modules(code)
        for value, at in lits:
            if any(a <= at < b for a, b in tests):
                continue
            text = normalise(value)
            if text is not None and text not in found and text not in SKIP_TEXT:
                found[text] = f'{path}:{src.count(chr(10), 0, at) + 1}'
    return found


# --- the language files -----------------------------------------------------

def escape(text):
    return text.replace('\\', '\\\\').replace('\n', '\\n')


def unescape(text):
    return re.sub(r'\\(.)', lambda m: '\n' if m.group(1) == 'n' else m.group(1), text)


def file_path(lang):
    return os.path.join(FILES_DIR, lang + '.txt')


def read_file(lang):
    """(header lines, OrderedDict english -> translation) of LANG's file."""
    path = file_path(lang)
    header, pairs, english = [], OrderedDict(), None
    if not os.path.exists(path):
        return header, pairs
    for line in open(path, encoding='utf-8').read().splitlines():
        if line.startswith('en: '):
            english = unescape(line[4:])
        elif line.startswith(lang + ': ') and english is not None:
            pairs[english] = unescape(line[len(lang) + 2:])
            english = None
        elif line.startswith('#') and not pairs:
            header.append(line)
    return header, pairs


def write_file(lang, header, pairs):
    os.makedirs(FILES_DIR, exist_ok=True)
    lines = header + ['']
    for english, translated in pairs.items():
        lines += ['en: ' + escape(english), lang + ': ' + escape(translated), '']
    # LF, like the other files git keeps; the Rust reader takes either.
    with open(file_path(lang), 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(lines))


def default_header(lang, glossary):
    name = LANGUAGES[lang]
    lines = [
        f'# GolemSculpt tooltips in {name} ({lang}).',
        '#',
        f'# Each tooltip is a pair of lines: "en:" is the English the app has, "{lang}:"',
        '# is what the tooltip shows instead. To fix a translation, edit its',
        f'# "{lang}:" line. Words in {{braces}} are filled in by the app and must stay;',
        '# \\n is a line break. Names of buttons and cards stay in English, as they',
        '# are on screen. tools/translations.py adds what is new.',
    ]
    if glossary:
        lines += ['#', '# The words used throughout:']
        lines += ['#   ' + g for g in glossary]
    return lines


# --- checking -------------------------------------------------------------

KEY = re.compile(r'\b(?:Ctrl|Shift|Alt)\s*\+\s*(?:[A-Z0-9=\-]\b|[A-Z][a-z]+(?: [A-Z][a-z]+)?|[=\-])|\bPage (?:Up|Down)\b|\bF\d{1,2}\b|\bEsc\b')
NAME = re.compile(r"\b[A-Z][\w'-]*(?: (?:to|of|the|and|in|on|a|as|after|for|by|&) [A-Z][\w'-]*| [A-Z][\w'-]*)+")
CJK = re.compile(r'[\u3040-\u30ff\u3400-\u9fff\uac00-\ud7af]')


def check_one(lang, english, translated):
    errors, warnings = [], []
    if not isinstance(translated, str) or not translated.strip():
        return ['empty'], []
    slots = sorted(re.findall(r'\{\w*\}', english))
    got = sorted(re.findall(r'\{\w*\}', translated))
    if slots != got:
        errors.append(f'placeholders {slots} became {got}')
    if english.count('\n') != translated.count('\n'):
        errors.append('a different number of line breaks')
    for key in set(KEY.findall(english)):
        squeezed = re.sub(r'\s+', '', key)
        if squeezed not in re.sub(r'\s+', '', translated):
            errors.append(f'key "{key}" is missing')
    for name in set(NAME.findall(english)):
        first = name.split(' ')[0]
        if name not in translated and not (english.startswith(name) and first in ('The', 'A', 'An', 'This', 'Each', 'Every')):
            warnings.append(f'on-screen name "{name}" is not kept as it is')
    ordinary = len(re.findall(r'(?<![{\w])[a-z]{2,}\b', english))
    if lang in ('ja', 'zh', 'ko') and ordinary >= 3 and not CJK.search(translated):
        errors.append('no characters of the language')
    if translated.strip() == english.strip() and len(english) > 20:
        warnings.append('left in English')
    return errors, warnings


def load_chunks(directory):
    by_id = {}
    for f in sorted(os.listdir(directory)):
        if f.startswith('chunk_') and f.endswith('.json'):
            for item in json.load(open(os.path.join(directory, f), encoding='utf-8')):
                by_id[str(item['id'])] = item['en']
    return by_id


def results(paths):
    out = {}
    for p in paths:
        try:
            data = json.load(open(p, encoding='utf-8'))
        except json.JSONDecodeError as e:
            sys.exit(f'{p}: not valid JSON: {e}')
        out[p] = {str(k): v for k, v in data.items()}
    return out


def chunk_ids(directory, result_path):
    """The ids of the chunk a result file answers, by its file name."""
    name = os.path.basename(result_path)
    path = os.path.join(directory, name)
    if os.path.exists(path):
        return [str(item['id']) for item in json.load(open(path, encoding='utf-8'))]
    return None


def take(args, flag):
    if flag in args:
        i = args.index(flag)
        value = args[i + 1]
        del args[i:i + 2]
        return value
    return None


def main(argv):
    if not argv:
        sys.exit(__doc__)
    cmd, args = argv[0], argv[1:]
    if cmd == 'status':
        found = sentences()
        print(f'{len(found)} sentences, {sum(map(len, found))} characters')
        for lang in LANGUAGES:
            _, pairs = read_file(lang)
            have = sum(1 for e in found if e in pairs)
            stale = sum(1 for e in pairs if e not in found)
            print(f'  {lang}: {have} translated, {len(found) - have} missing, {stale} no longer in the source')
    elif cmd == 'todo':
        out = args[0]
        size = int(args[1]) if len(args) > 1 else 200
        found = list(sentences())
        missing = [e for e in found if any(e not in read_file(l)[1] for l in LANGUAGES)]
        os.makedirs(out, exist_ok=True)
        items = [{'id': found.index(e), 'en': e} for e in missing]
        for n in range(0, len(items), size):
            with open(os.path.join(out, f'chunk_{n // size + 1:02}.json'), 'w', encoding='utf-8') as f:
                json.dump(items[n:n + size], f, ensure_ascii=False, indent=1)
        print(f'{len(items)} sentences in {(len(items) + size - 1) // size} chunks')
    elif cmd in ('check', 'merge'):
        chunks = take(args, '--chunks')
        glossary = take(args, '--glossary')
        lang = args[0]
        if lang not in LANGUAGES or chunks is None:
            sys.exit('usage: check|merge LANG --chunks DIR RESULT.json...')
        english = load_chunks(chunks)
        bad = 0
        merged = {}
        for path, data in results(args[1:]).items():
            expected = chunk_ids(chunks, path)
            if expected is not None:
                for i in expected:
                    if i not in data:
                        print(f'{path}: id {i} has no translation')
                        bad += 1
            for i, translated in data.items():
                if i not in english:
                    print(f'{path}: id {i} is not in any chunk')
                    bad += 1
                    continue
                errors, warnings = check_one(lang, english[i], translated)
                for e in errors:
                    print(f'ERROR {path} id {i}: {e}\n    en: {english[i]}\n    {lang}: {translated}')
                for w in warnings:
                    if cmd == 'check':
                        print(f'warning {path} id {i}: {w}')
                bad += len(errors)
                if not errors:
                    merged[english[i]] = translated
        if cmd == 'check':
            print(f'{bad} errors' if bad else 'no errors')
            sys.exit(1 if bad else 0)
        header, pairs = read_file(lang)
        pairs.update(merged)
        terms = [l.strip() for l in open(glossary, encoding='utf-8') if l.strip()] if glossary else None
        if terms is not None or not header:
            header = default_header(lang, terms or [])
        order = list(sentences())
        place = {e: i for i, e in enumerate(order)}
        # A sentence no longer in the source can never be looked up: dropped.
        kept = [kv for kv in pairs.items() if kv[0] in place]
        ordered = OrderedDict(sorted(kept, key=lambda kv: place[kv[0]]))
        write_file(lang, header, ordered)
        print(f'{lang}: {len(merged)} merged, {len(ordered)} in the file, {len(pairs) - len(kept)} no longer in the source dropped, {bad} left out with errors')
    else:
        sys.exit(__doc__)


if __name__ == '__main__':
    sys.stdout.reconfigure(encoding='utf-8')
    main(sys.argv[1:])
