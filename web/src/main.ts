// Name 100: the game. One input, one list and a timer; see name100-frontend.md.

import "./style.css";
import { categoriesById, lookup, type Category, type LookupOutcome } from "./api";
import { installTooltip } from "./charts";
import { confirmDialog, dialogOpen } from "./dialog";
import { h, rich } from "./dom";
import * as messages from "./messages";
import { createPicker } from "./picker";
import { entryItem, renderResults, type Results } from "./results";
import {
  bestTime,
  entryFrom,
  formatTime,
  gameKey,
  loadGame,
  newGame,
  recordTime,
  saveGame,
  TARGET,
  WOMEN,
  type Game,
} from "./store";
import { gameParts, gameText, peopleWord, sizeNote } from "./wording";

/** A lookup still unanswered after this long shows "Checking…". */
const SLOW_MS = 400;
const MAX_CATEGORIES = 2;

let game: Game = newGame([WOMEN]);
/** When the running clock was last started, on the `performance.now()` scale. */
let clockStart: number | null = null;
/** Bumped by every new game, so answers to an old game's guesses are dropped. */
let generation = 0;
/** Lookups are sent at once but answered in the order they were sent. */
let answers: Promise<void> = Promise.resolve();
let picker: HTMLElement | null = null;
let results: Results | null = null;

// ---- The page ----

const title = h("h1", { class: "title" });
const sub = h("p", { class: "sub" });
const pickerSlot = h("div");
const countNow = h("span", { class: "count-now" }, "0");
const timer = h("span", { class: "timer", role: "timer", "aria-label": "Time" }, "0:00");
const progress = h("div", { class: "progress-fill" });
const input = h("input", {
  class: "guess",
  type: "text",
  name: "guess",
  "aria-label": "Name",
  placeholder: "Type a name, press Enter",
  autocomplete: "off",
  autocorrect: "off",
  autocapitalize: "off",
  spellcheck: "false",
  enterkeyhint: "go",
  maxlength: 100,
});
const form = h("form", { class: "guess-form" }, input);
const message = h("p", { class: "message", role: "status", "aria-live": "polite" });
const names = h("ol", { class: "names" });
const finishButton = h("button", { type: "button", class: "link" }, "Finish");
const newButton = h("button", { type: "button", class: "link" }, "New game");
const controls = h("div", { class: "controls" }, finishButton, newButton);
const play = h("div", { class: "play" }, form, message, names, controls);
const resultSlot = h("div");
const scoreboard = h(
  "div",
  {},
  h(
    "div",
    { class: "score" },
    h("span", { class: "count", "aria-label": "Named" }, countNow, h("span", { class: "count-of" }, ` / ${TARGET}`)),
    timer,
  ),
  h("div", { class: "progress" }, progress),
);

document.getElementById("app")!.append(
  h("header", { class: "top" }, title, sub, pickerSlot),
  scoreboard,
  play,
  resultSlot,
  h(
    "footer",
    { class: "foot" },
    "Names are checked against ",
    h("a", { href: "https://www.wikidata.org/", target: "_blank", rel: "noopener" }, "Wikidata"),
    ". It is sometimes wrong or out of date.",
  ),
);

// ---- Clock and storage ----

const elapsed = (): number =>
  game.elapsedMs + (clockStart === null ? 0 : performance.now() - clockStart);

/** Fold the running clock into the game and save it. */
function save(): void {
  if (clockStart !== null) {
    const now = performance.now();
    game.elapsedMs = Math.round(game.elapsedMs + now - clockStart);
    clockStart = now;
  }
  saveGame(game);
}

function setMessage(text: messages.Text | null, quiet = false): void {
  message.classList.toggle("quiet", quiet);
  message.replaceChildren(...(text ? rich(text) : []));
}

function focusInput(): void {
  if (!game.finished && !picker && !dialogOpen()) input.focus({ preventScroll: true });
}

// ---- Drawing ----

function drawTitle(): void {
  title.replaceChildren(
    `Name ${TARGET} `,
    ...gameParts(game.categories).map((part) =>
      typeof part === "string"
        ? part
        : h(
            "button",
            { type: "button", class: "term", title: "Change category", onclick: () => openPicker(part.term) },
            part.text,
          ),
    ),
  );
  document.title = `Name ${TARGET} ${gameText(game.categories)}`;

  const size = sizeNote(game.categories);
  const best = bestTime(game.categories);
  const items: (Node | string)[] = [];
  if (size) items.push(size.small ? `${size.text} to choose from, so ${TARGET} may be out of reach` : `${size.text} to choose from`);
  if (best !== null) items.push(`Best ${formatTime(best)}`);
  if (game.categories.length < MAX_CATEGORIES) {
    items.push(h("button", { type: "button", class: "link", onclick: () => openPicker(null) }, "+ add another"));
  }
  sub.replaceChildren(...items.map((item) => (typeof item === "string" ? h("span", {}, item) : item)));
}

function drawScore(): void {
  const count = game.entries.length;
  countNow.textContent = String(count);
  progress.style.width = `${Math.min(100, (count / TARGET) * 100)}%`;
  timer.textContent = formatTime(elapsed());
  finishButton.hidden = count === 0;
  newButton.hidden = count === 0 && !game.running;
  controls.hidden = finishButton.hidden && newButton.hidden;
}

function drawNames(): void {
  names.replaceChildren(
    ...game.entries.map((entry, position) => entryItem(entry, position + 1, false)).reverse(),
  );
}

function drawMode(): void {
  play.hidden = game.finished;
  // The result screen states the count and time itself.
  scoreboard.hidden = game.finished;
  input.disabled = game.finished;
  if (game.finished) {
    results = renderResults(game, bestTime(game.categories), { onNewGame: () => void askNewGame() });
    resultSlot.replaceChildren(results.element);
    results.layout();
  } else {
    results = null;
    resultSlot.replaceChildren();
  }
}

function drawAll(): void {
  drawTitle();
  drawScore();
  drawNames();
  drawMode();
}

new ResizeObserver(() => results?.layout()).observe(resultSlot);

// ---- Playing ----

function start(next: Game): void {
  generation += 1;
  game = next;
  clockStart = game.running ? performance.now() : null;
  closePicker();
  setMessage(null);
  input.value = "";
  syncAddress();
  saveGame(game);
  drawAll();
  focusInput();
}

function finish(): void {
  save();
  clockStart = null;
  game.running = false;
  game.finished = true;
  if (game.entries.length >= TARGET) recordTime(game.categories, game.elapsedMs);
  saveGame(game);
  setMessage(null);
  drawAll();
  window.scrollTo({ top: 0 });
}

function answer(text: string, outcome: LookupOutcome): void {
  if (outcome.kind === "failed") {
    // The guess goes back in the input unless the player is already typing the next.
    if (input.value === "") input.value = text;
    setMessage(messages.COULD_NOT_CHECK);
    return;
  }
  if (outcome.kind === "not_a_name") {
    game.rejected += 1;
    setMessage(messages.NOT_A_NAME);
    return;
  }
  const { response } = outcome;
  const entry = response.verdict === "accepted" ? entryFrom(response, elapsed()) : null;
  if (!entry) {
    game.rejected += 1;
    setMessage(messages.rejection(response));
    return;
  }
  if (game.entries.some((named) => named.qid === entry.qid)) {
    game.rejected += 1;
    setMessage(messages.alreadyNamed(entry.name));
    return;
  }
  game.entries.push(entry);
  setMessage(null);
  save();
  if (game.entries.length >= TARGET) {
    finish();
    return;
  }
  drawScore();
  names.prepend(entryItem(entry, game.entries.length, false));
  names.firstElementChild?.classList.add("fresh");
}

function submit(): void {
  const text = input.value.trim();
  if (text === "" || game.finished) return;
  input.value = "";
  if (!game.running) {
    game.running = true;
    clockStart = performance.now();
    drawScore();
  }
  const sent = generation;
  const pending = lookup(text, game.categories.map((c) => c.id));
  const slow = window.setTimeout(() => setMessage(["Checking…"], true), SLOW_MS);
  answers = answers.then(async () => {
    const outcome = await pending;
    window.clearTimeout(slow);
    if (sent !== generation || game.finished) return;
    answer(text, outcome);
  });
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  submit();
  focusInput();
});

finishButton.addEventListener("click", finish);
newButton.addEventListener("click", () => void askNewGame());

/** Whether leaving the current game would lose anything. */
const inProgress = (): boolean => !game.finished && game.entries.length > 0;

async function confirmLeaving(question: string, yes: string): Promise<boolean> {
  if (!inProgress()) return true;
  const count = game.entries.length;
  const word = peopleWord(game.categories);
  const sure = await confirmDialog(
    question,
    `The ${count === 1 ? "one name" : `${count} ${word}`} you have so far will be cleared.`,
    yes,
  );
  if (!sure) focusInput();
  return sure;
}

async function askNewGame(): Promise<void> {
  if (await confirmLeaving("Start a new game?", "New game")) start(newGame(game.categories));
}

// ---- Choosing a category ----

function closePicker(): void {
  picker?.remove();
  picker = null;
}

async function changeCategories(next: Category[]): Promise<void> {
  if (gameKey(next) === gameKey(game.categories)) return closePicker();
  if (await confirmLeaving("Change the game?", "Change")) {
    start(newGame(next));
  } else {
    closePicker();
    focusInput();
  }
}

/** Open the search to replace category `slot`, or to add one when `slot` is null. */
function openPicker(slot: number | null): void {
  closePicker();
  const current = game.categories;
  const replaced = slot === null ? null : (current[slot] ?? null);
  picker = createPicker({
    exclude: current.map((c) => c.id),
    removable: replaced && current.length > 1 ? replaced : null,
    onPick: (category) =>
      void changeCategories(
        replaced ? current.map((c) => (c === replaced ? category : c)) : [...current, category],
      ),
    onRemove: () => void changeCategories(current.filter((c) => c !== replaced)),
    onClose: () => {
      closePicker();
      focusInput();
    },
  });
  pickerSlot.append(picker);
}

// ---- The page address: ?category=women,occupation:Q33999 ----

function addressIds(): string[] {
  const param = new URLSearchParams(location.search).get("category") ?? "";
  return [...new Set(param.split(",").map((id) => id.trim()).filter(Boolean))].slice(0, MAX_CATEGORIES);
}

function syncAddress(): void {
  const url = new URL(location.href);
  const key = game.categories.map((c) => c.id).join(",");
  if (key === "women") url.searchParams.delete("category");
  else url.searchParams.set("category", key);
  // Keep the ids readable in the address bar.
  history.replaceState(null, "", url.pathname + url.search.replace(/%3A/gi, ":").replace(/%2C/gi, ",") + url.hash);
}

// ---- Keeping the input focused, and the clock honest ----

document.addEventListener("click", (event) => {
  const target = event.target as Element;
  if (!target.closest("button, a, input, dialog, .picker") && !window.getSelection()?.toString()) focusInput();
});
document.addEventListener("keydown", (event) => {
  const typing = event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
  if (typing && document.activeElement === document.body) focusInput();
});
window.setInterval(() => {
  if (clockStart !== null) timer.textContent = formatTime(elapsed());
}, 200);
window.setInterval(() => {
  if (clockStart !== null) save();
}, 5000);
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden" && !game.finished) save();
});
window.addEventListener("pagehide", () => {
  if (!game.finished) save();
});

// ---- Start ----

/** Refresh the saved categories' names and sizes; they change when the index is rebuilt. */
async function refreshCategories(): Promise<void> {
  const mine = game;
  try {
    const fresh = await categoriesById(mine.categories.map((c) => c.id));
    if (mine !== game || fresh.length !== mine.categories.length) return;
    mine.categories = fresh;
    saveGame(mine);
    drawTitle();
  } catch {
    // The server is away; the first guess will say so.
  }
}

async function boot(): Promise<void> {
  installTooltip();
  const saved = loadGame();
  const linked = addressIds();
  game = saved ?? game;
  clockStart = game.running ? performance.now() : null;
  drawAll();
  focusInput();

  const sameGame = linked.length === 0 || [...linked].sort().join(",") === gameKey(game.categories);
  if (!sameGame) {
    // The address names another game than the saved one.
    let wanted: Category[] = [];
    try {
      wanted = await categoriesById(linked);
    } catch {
      setMessage(["Can't reach the lookup server."]);
    }
    if (wanted.length === linked.length) {
      if (await confirmLeaving(`Play “Name ${TARGET} ${gameText(wanted)}”?`, "Play it")) {
        start(newGame(wanted));
        return;
      }
    } else if (wanted.length > 0 || linked.length > 0) {
      if (message.childNodes.length === 0) setMessage(["That link's category isn't one this game knows."]);
    }
    syncAddress();
  }
  void refreshCategories();
}

void boot();
