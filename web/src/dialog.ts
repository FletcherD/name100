// A yes-or-no question in a modal dialog. Escape and the backdrop mean no.

import { h } from "./dom";

export function confirmDialog(question: string, detail: string, yes: string, no = "Cancel"): Promise<boolean> {
  return new Promise((resolve) => {
    const dialog = h(
      "dialog",
      { class: "confirm" },
      h("h2", {}, question),
      detail ? h("p", {}, detail) : null,
      h(
        "form",
        { method: "dialog", class: "actions" },
        h("button", { value: "no", class: "button quiet" }, no),
        h("button", { value: "yes", class: "button", autofocus: true }, yes),
      ),
    );
    dialog.addEventListener("click", (event) => {
      if (event.target === dialog) dialog.close("no");
    });
    dialog.addEventListener("close", () => {
      dialog.remove();
      resolve(dialog.returnValue === "yes");
    });
    document.body.append(dialog);
    dialog.showModal();
  });
}

export const dialogOpen = (): boolean => document.querySelector("dialog[open]") !== null;
