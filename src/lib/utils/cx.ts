type ClassName = string | false | null | undefined;

/**
 * Build a class-name joiner for a CSS module.
 *
 * Each name is emitted as-is (so global rules such as `.btn` apply) plus its
 * module-scoped counterpart when the module defines one. This mirrors how the
 * Svelte components combined global and scoped styles.
 */
export function scoped(styles: Readonly<Record<string, string>>) {
  return (...names: ClassName[]): string =>
    names
      .filter((name): name is string => Boolean(name))
      .flatMap((name) => {
        const local = styles[name];
        return local ? [name, local] : [name];
      })
      .join(" ");
}
