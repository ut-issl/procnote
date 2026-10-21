import DOMPurify from "dompurify";
import { Marked } from "marked";
import { markedHighlight } from "marked-highlight";
import hljs from "highlight.js/lib/core";

import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import bash from "highlight.js/lib/languages/bash";
import yaml from "highlight.js/lib/languages/yaml";
import json from "highlight.js/lib/languages/json";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import toml from "highlight.js/lib/languages/ini";
import sql from "highlight.js/lib/languages/sql";
import xml from "highlight.js/lib/languages/xml";
import css_lang from "highlight.js/lib/languages/css";
import markdown_lang from "highlight.js/lib/languages/markdown";

import "highlight.js/styles/atom-one-light.css";

hljs.registerLanguage("python", python);
hljs.registerLanguage("rust", rust);
hljs.registerLanguage("bash", bash);
hljs.registerLanguage("shell", bash);
hljs.registerLanguage("sh", bash);
hljs.registerLanguage("yaml", yaml);
hljs.registerLanguage("yml", yaml);
hljs.registerLanguage("json", json);
hljs.registerLanguage("javascript", javascript);
hljs.registerLanguage("js", javascript);
hljs.registerLanguage("typescript", typescript);
hljs.registerLanguage("ts", typescript);
hljs.registerLanguage("toml", toml);
hljs.registerLanguage("sql", sql);
hljs.registerLanguage("xml", xml);
hljs.registerLanguage("html", xml);
hljs.registerLanguage("css", css_lang);
hljs.registerLanguage("markdown", markdown_lang);
hljs.registerLanguage("md", markdown_lang);

const markedInstance = new Marked(
  markedHighlight({
    langPrefix: "hljs language-",
    highlight(code, lang) {
      const language = hljs.getLanguage(lang) ? lang : "plaintext";
      return hljs.highlight(code, { language }).value;
    },
  }),
);

const markdownCache = new Map<string, string>();

/** Render step prose to sanitized HTML with highlighted code blocks. */
export function renderMarkdown(source: string): string {
  const cached = markdownCache.get(source);
  if (cached) return cached;
  const rendered = DOMPurify.sanitize(markedInstance.parse(source, { async: false }) as string);
  if (markdownCache.size > 500) markdownCache.clear();
  markdownCache.set(source, rendered);
  return rendered;
}

/** Render a table cell's inline Markdown to sanitized HTML. */
export function renderInlineMarkdown(source: string): string {
  return DOMPurify.sanitize(markedInstance.parseInline(source, { async: false }) as string);
}
