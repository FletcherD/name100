// The category search: type "actors", pick from the matches.

import { searchCategories, type Category } from "./api";
import { h } from "./dom";
import { formatCount, kindName } from "./wording";

/** How long to wait after the last keystroke before searching. */
const DEBOUNCE_MS = 150;

export interface PickerOptions {
  /** Ids left out of the matches: the categories already in the game. */
  exclude: readonly string[];
  /** The category this search replaces, when it may also simply be removed. */
  removable: Category | null;
  onPick(category: Category): void;
  onRemove(): void;
  onClose(): void;
}

export function createPicker(options: PickerOptions): HTMLElement {
  let found: Category[] = [];
  let active = 0;
  let timer: number | undefined;
  let request: AbortController | undefined;

  const input = h("input", {
    type: "search",
    class: "picker-input",
    placeholder: "Search categories: actors, France, tennis…",
    "aria-label": "Search categories",
    "aria-controls": "picker-list",
    "aria-autocomplete": "list",
    role: "combobox",
    "aria-expanded": "true",
    autocomplete: "off",
    autocapitalize: "off",
    spellcheck: "false",
  });
  const list = h("ul", { class: "picker-list", id: "picker-list", role: "listbox" });
  const status = h("p", { class: "picker-status", role: "status" });

  const draw = () => {
    list.replaceChildren(
      ...found.map((category, position) =>
        h(
          "li",
          {
            role: "option",
            id: `picker-option-${position}`,
            "aria-selected": position === active ? "true" : "false",
            onclick: () => options.onPick(category),
            onpointermove: () => {
              if (active !== position) {
                active = position;
                draw();
              }
            },
          },
          h("span", { class: "picker-name" }, category.label),
          h(
            "span",
            { class: "picker-meta" },
            `${kindName(category.kind)} · ${formatCount(category.people)} people`,
          ),
        ),
      ),
    );
    if (found.length > 0) input.setAttribute("aria-activedescendant", `picker-option-${active}`);
    else input.removeAttribute("aria-activedescendant");
  };

  const search = async () => {
    request?.abort();
    request = new AbortController();
    const text = input.value.trim();
    try {
      const all = await searchCategories(text, request.signal);
      found = all.filter((category) => !options.exclude.includes(category.id));
      active = 0;
      status.textContent = found.length === 0 ? `No category matches “${text}”.` : "";
      draw();
    } catch (error) {
      if ((error as Error).name === "AbortError") return;
      found = [];
      status.textContent = "Can't reach the lookup server.";
      draw();
    }
  };

  input.addEventListener("input", () => {
    window.clearTimeout(timer);
    timer = window.setTimeout(search, DEBOUNCE_MS);
  });
  input.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (found.length === 0) return;
      active = (active + (event.key === "ArrowDown" ? 1 : found.length - 1)) % found.length;
      draw();
      list.children[active]?.scrollIntoView({ block: "nearest" });
    } else if (event.key === "Enter") {
      event.preventDefault();
      const choice = found[active];
      if (choice) options.onPick(choice);
    } else if (event.key === "Escape") {
      event.preventDefault();
      options.onClose();
    }
  });

  const panel = h(
    "div",
    { class: "picker" },
    input,
    list,
    status,
    h(
      "div",
      { class: "picker-foot" },
      options.removable
        ? h("button", { type: "button", class: "link", onclick: options.onRemove }, `Remove “${options.removable.label}”`)
        : h("span"),
      h("button", { type: "button", class: "link", onclick: options.onClose }, "Cancel"),
    ),
  );
  void search();
  queueMicrotask(() => input.focus());
  return panel;
}
