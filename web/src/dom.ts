// Small helpers for building elements without a framework.

type Child = Node | string | null | undefined | false;
type Attrs = Record<string, string | number | boolean | null | undefined | ((event: never) => void)>;

function apply(element: Element, attrs: Attrs, children: Child[]): void {
  for (const [key, value] of Object.entries(attrs)) {
    if (value === null || value === undefined || value === false) continue;
    if (typeof value === "function") {
      element.addEventListener(key.slice(2), value as EventListener);
    } else {
      element.setAttribute(key, value === true ? "" : String(value));
    }
  }
  element.append(...children.filter((child): child is Node | string => Boolean(child)));
}

/** `h("button", { class: "link", onclick: fn }, "Finish")` */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  apply(element, attrs, children);
  return element;
}

/** The same for SVG elements. */
export function s(tag: string, attrs: Attrs = {}, ...children: Child[]): SVGElement {
  const element = document.createElementNS("http://www.w3.org/2000/svg", tag);
  apply(element, attrs, children);
  return element;
}

/** Message text as nodes: `em` parts become `<em>`. */
export function rich(text: readonly (string | { em: string })[]): Node[] {
  return text.map((part) =>
    typeof part === "string" ? document.createTextNode(part) : h("em", {}, part.em),
  );
}
