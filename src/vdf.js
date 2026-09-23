'use strict';

/**
 * Parser for Valve's text KeyValues format (.vdf / .acf):
 *
 *   "AppState" { "appid" "620" "name" "Portal 2" }
 *
 * Returns nested plain objects. Keys keep their original case; use `get()` for case-insensitive lookups,
 * since Steam is inconsistent ("apps" vs "Apps").
 */
function parse(text) {
  let i = 0;
  const n = text.length;

  function skip() {
    while (i < n) {
      const c = text[i];
      if (c === ' ' || c === '\t' || c === '\r' || c === '\n' || c === '﻿') i++;
      else if (c === '/' && text[i + 1] === '/') {
        while (i < n && text[i] !== '\n') i++;
      } else break;
    }
  }

  function token() {
    skip();
    if (i >= n) return null;
    const c = text[i];
    if (c === '{' || c === '}') {
      i++;
      return { type: c };
    }
    if (c === '"') {
      i++;
      let out = '';
      while (i < n && text[i] !== '"') {
        if (text[i] === '\\' && i + 1 < n) {
          const e = text[i + 1];
          out += e === 'n' ? '\n' : e === 't' ? '\t' : e;
          i += 2;
        } else out += text[i++];
      }
      i++; // closing quote
      return { type: 'str', value: out };
    }
    // Unquoted token (rare, but valid).
    let out = '';
    while (i < n && !/[\s{}"]/.test(text[i])) out += text[i++];
    return { type: 'str', value: out };
  }

  function object(nested) {
    const obj = {};
    for (;;) {
      const k = token();
      if (!k) return obj;
      if (k.type === '}') {
        if (nested) return obj;
        continue;
      }
      if (k.type !== 'str') continue;
      const v = token();
      if (!v) return obj;
      // Skip conditionals like [$WIN32] that may follow a value.
      skip();
      if (text[i] === '[') {
        while (i < n && text[i] !== ']') i++;
        i++;
      }
      if (v.type === '{') obj[k.value] = object(true);
      else if (v.type === 'str') obj[k.value] = v.value;
    }
  }

  return object(false);
}

/** Case-insensitive property lookup along a path: get(o, 'UserLocalConfigStore', 'Software', 'Valve'). */
function get(obj, ...keys) {
  let cur = obj;
  for (const key of keys) {
    if (!cur || typeof cur !== 'object') return undefined;
    if (key in cur) {
      cur = cur[key];
      continue;
    }
    const lower = String(key).toLowerCase();
    const hit = Object.keys(cur).find((k) => k.toLowerCase() === lower);
    cur = hit === undefined ? undefined : cur[hit];
  }
  return cur;
}

module.exports = { parse, get };
