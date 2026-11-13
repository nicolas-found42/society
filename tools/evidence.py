#!/usr/bin/env python3
"""Read retained Society evidence without a running app. Python standard library only."""
import argparse
from collections import deque
from datetime import datetime
import json
from pathlib import Path
import re
import sys

MAX_LINE = 1024 * 1024
MAX_RESULT = 8 * 1024 * 1024


def limit_value(value):
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError('must be >= 0; the server rejects negative limits')
    return number


def instant(value):
    if value.isdigit():
        return int(value)
    parsed = datetime.fromisoformat(value.replace('Z', '+00:00'))
    if parsed.tzinfo is None:
        raise argparse.ArgumentTypeError('include a UTC offset or Z')
    return int(parsed.timestamp())


def query(save, journal=False, since=None, until=None, world=None, operation=None,
          session=None, since_turn=None, limit=200, crash=False):
    head = Path(str(save) + ('.crash.json' if crash else '.journal.jsonl' if journal else '.diagnostics.jsonl'))
    result = {'entries': [], 'files': [], 'corrupt_lines': 0, 'read_errors': [],
              'head_missing': False, 'truncated': False, 'uncorrelated_rows': 0,
              'coverage': 'retained files only; no age guarantee; absence is not proof of no operation'}
    rows = deque()
    row_bytes = 0
    # The server rejects a negative limit with HTTP 400; `main` rejects it too,
    # this clamp only bounds programmatic callers.
    limit = min(max(limit, 0), 1000)
    paths = []
    # The crash record is written whole and never rotated, so a same-stem
    # numbered file beside it is a foreign artifact, not retained history.
    pattern = re.compile(re.escape(head.stem) + r'\.(\d+)\.jsonl$')
    try:
        for candidate in head.parent.iterdir():
            match = None if crash else pattern.fullmatch(candidate.name)
            # The server reads canonical decimal slots only: a padded name such as
            # `.01` is a foreign artifact on both sides.
            if match and not (len(match[1]) > 1 and match[1].startswith('0')):
                paths.append((int(match[1]), candidate))
    except FileNotFoundError:
        result['head_missing'] = True
    except OSError as err:
        result['read_errors'].append({'file': str(head.parent), 'kind': type(err).__name__})
    paths = [p for _, p in sorted(paths, reverse=True)] + [head]
    for path in paths:
        try:
            with path.open('rb') as stream:
                result['files'].append(str(path))
                while True:
                    line = stream.readline(MAX_LINE + 1)
                    if not line:
                        break
                    if len(line) > MAX_LINE:
                        while line and not line.endswith(b'\n'):
                            line = stream.readline(MAX_LINE + 1)
                        result['corrupt_lines'] += 1
                        continue
                    if not line.strip():
                        continue
                    try:
                        value = json.loads(line)
                        # bool is an int subclass in Python; the server's u64 does
                        # not accept `true`, so neither may this reader.
                        if not isinstance(value, dict) or type(value.get('wall_time')) is not int:
                            raise ValueError('missing timestamp')
                        context = value.get('context') or {}
                        if not isinstance(context, dict):
                            raise ValueError('invalid context')
                        if not context.get('world_id'):
                            result['uncorrelated_rows'] += 1
                        if since is not None and value['wall_time'] < since:
                            continue
                        if until is not None and value['wall_time'] > until:
                            continue
                        # Only the journal is ordered by turn; the audit and
                        # crash files have no turn-filtered HTTP counterpart.
                        if journal and since_turn is not None and value.get('turn', -1) < since_turn:
                            continue
                        if any(expected is not None and context.get(key) != expected for key, expected in
                               [('world_id', world), ('operation_id', operation), ('session_id', session)]):
                            continue
                        rows.append((value, len(line)))
                        row_bytes += len(line)
                        while len(rows) > limit or row_bytes > MAX_RESULT:
                            _, size = rows.popleft()
                            row_bytes -= size
                            result['truncated'] = True
                    except (ValueError, UnicodeError, TypeError):
                        result['corrupt_lines'] += 1
        except FileNotFoundError:
            if path == head:
                result['head_missing'] = True
            else:
                result['read_errors'].append({'file': str(path), 'kind': 'disappeared_during_read'})
        except OSError as err:
            result['read_errors'].append({'file': str(path), 'kind': type(err).__name__})
    result['entries'] = [value for value, _ in rows]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--save', default='data/society.json')
    stream = parser.add_mutually_exclusive_group()
    stream.add_argument('--journal', action='store_true', help='turn records, including spoken text; handle as sensitive')
    stream.add_argument('--crash', action='store_true', help='latest panic location; one retained record')
    parser.add_argument('--since', type=instant, help='inclusive epoch seconds or ISO time with offset')
    parser.add_argument('--until', type=instant, help='inclusive epoch seconds or ISO time with offset')
    parser.add_argument('--world')
    parser.add_argument('--operation')
    parser.add_argument('--session')
    parser.add_argument('--since-turn', type=int,
                        help='journal only: drop entries committed before this turn')
    parser.add_argument('--limit', type=limit_value, default=200)
    args = vars(parser.parse_args())
    # An empty identity filter is the absence of a filter, matching the HTTP
    # handlers: a script that defaults a variable to "" must not invert it into
    # "only legacy contextless rows".
    for key in ('world', 'operation', 'session'):
        if args[key] == '':
            args[key] = None
    result = query(**args)
    json.dump(result, sys.stdout, indent=2)
    print()
    return 2 if result['read_errors'] or result['corrupt_lines'] else 0


if __name__ == '__main__':
    sys.exit(main())
