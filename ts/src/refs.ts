/** One `$` form found in an authored string (SPEC §6). */
export interface Reference {
  name: string;
  hasDefault: boolean;
}

const NAME_START = /[A-Za-z_]/;
const NAME_CHAR = /[A-Za-z0-9_]/;

/**
 * The variable references of an authored string in order of appearance. Used
 * for provenance of a value that already loaded, so malformed forms are simply
 * skipped rather than reported.
 */
export function scanReferences(s: string): Reference[] {
  const out: Reference[] = [];
  let i = 0;
  while (i < s.length) {
    if (s[i] !== "$" || i + 1 >= s.length) {
      i++;
      continue;
    }
    const next = s[i + 1];
    if (next === "$") {
      i += 2;
    } else if (next === "{") {
      const end = s.indexOf("}", i + 2);
      if (end === -1) return out;
      const inner = s.slice(i + 2, end);
      const colon = inner.indexOf(":");
      out.push(
        colon === -1
          ? { name: inner, hasDefault: false }
          : { name: inner.slice(0, colon), hasDefault: true },
      );
      i = end + 1;
    } else if (NAME_START.test(next)) {
      let j = i + 1;
      while (j < s.length && NAME_CHAR.test(s[j])) j++;
      out.push({ name: s.slice(i + 1, j), hasDefault: false });
      i = j;
    } else {
      i++;
    }
  }
  return out;
}
