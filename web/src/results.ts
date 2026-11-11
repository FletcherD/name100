// The result screen. Everything on it comes from the entries already held.

import type { Category } from "./api";
import { barChart, columnChart, numbersList, pieChart, sliceColor } from "./charts";
import { h } from "./dom";
import * as stats from "./stats";
import { formatTime, TARGET, type Entry, type Game } from "./store";
import { countryArticle, gameText, peopleWord } from "./wording";

export interface ResultHandlers {
  onNewGame(): void;
}

export interface Results {
  element: HTMLElement;
  /** Draw the charts at the element's current width; call again when it changes. */
  layout(): void;
}

/** The line "Copy result" puts on the clipboard: count and time, no names. */
export function resultLine(game: Game): string {
  return `Named ${game.entries.length} ${gameText(game.categories)} in ${formatTime(game.elapsedMs)}.`;
}

/** One accepted name as a list item; the result screen adds the time and a link. */
export function entryItem(entry: Entry, number: number, full: boolean): HTMLLIElement {
  const name =
    full && entry.url
      ? h("a", { class: "name", href: entry.url, target: "_blank", rel: "noopener" }, entry.name)
      : h("span", { class: "name" }, entry.name);
  const notes = [
    entry.alias ? h("span", {}, "as ", h("em", {}, entry.alias)) : null,
    entry.corrected ? h("span", {}, "spelling corrected") : null,
  ].filter(Boolean);
  return h(
    "li",
    {},
    h("span", { class: "num" }, String(number)),
    h(
      "div",
      { class: "who" },
      name,
      entry.description ? h("span", { class: "desc" }, ` [${entry.description}]`) : null,
      notes.length > 0 ? h("div", { class: "notes" }, ...notes) : null,
    ),
    full ? h("span", { class: "at" }, formatTime(entry.at)) : null,
  );
}

type Draw = (width: number) => Node[];

function figure(title: string, caption: string, draw: Draw | null, ...rest: Node[]): { node: HTMLElement; chart: HTMLElement | null; draw: Draw | null } {
  const chart = draw ? h("div", { class: "chart" }) : null;
  const node = h("figure", {}, h("h3", {}, title), chart, ...rest, h("figcaption", {}, caption));
  return { node, chart, draw };
}

const numbers = (...items: [number, string][]): HTMLElement =>
  h(
    "dl",
    { class: "numbers" },
    ...items.map(([value, label]) => h("div", {}, h("dt", {}, label), h("dd", {}, String(value)))),
  );

function period(label: string): string {
  return label.replace(" c.", " century");
}

export function renderResults(game: Game, best: number | null, handlers: ResultHandlers): Results {
  const { entries, categories } = game;
  const count = entries.length;
  const people = peopleWord(categories);
  const complete = count >= TARGET;
  const figures: ReturnType<typeof figure>[] = [];

  // What they do: left out when the game is itself about an occupation.
  if (count > 0 && !categories.some((c: Category) => c.kind === "occupation")) {
    const slices = stats.occupations(entries);
    const top = slices[0];
    const caption =
      top && top.label !== stats.OTHER && top.label !== stats.UNCATEGORIZED
        ? `${top.label} is the biggest group: ${top.count} of your ${count}.`
        : "No occupation group has three or more of them.";
    const legend = h(
      "ul",
      { class: "legend" },
      ...slices.map((slice, position) =>
        h(
          "li",
          {},
          h("span", { class: "swatch", style: `background:${sliceColor(slice, position)}` }),
          h("span", { class: "legend-name" }, slice.label),
          h("span", { class: "legend-count" }, String(slice.count)),
        ),
      ),
    );
    const about = h(
      "p",
      { class: "aside" },
      "Each person is counted once, under a main occupation picked from their Wikidata entry. It is a good guess, not a fact.",
    );
    const made = figure("What they do", caption, (width) => [pieChart(slices, Math.min(176, width))]);
    made.chart!.classList.add("pie");
    made.chart!.after(legend);
    made.node.classList.add("with-legend");
    made.node.append(about);
    figures.push(made);
  }

  if (count > 0) {
    const bars = stats.fame(entries);
    const middle = stats.median(entries.map((entry) => entry.sitelinks));
    figures.push(
      figure(
        "How famous",
        `Half of them have articles in ${middle} or more language editions of Wikipedia.`,
        (width) => [columnChart(bars, width)],
        numbersList(bars),
      ),
    );

    const born = stats.births(entries);
    if (born.bars.length > 0) {
      const top = born.bars.reduce((a, b) => (b.count > a.count ? b : a));
      const unknown =
        born.unknown > 0 ? ` ${born.unknown} ${born.unknown === 1 ? "has" : "have"} no birth year recorded.` : "";
      figures.push(
        figure(
          "When they were born",
          `The most common period is the ${period(top.label)}, with ${top.count}.${unknown}`,
          (width) => [columnChart(born.bars, width)],
          numbersList(born.bars),
        ),
      );
    }

    const { recorded, none } = stats.deaths(entries);
    figures.push(
      figure(
        "Living or not",
        "Wikidata does not record every death, so “no death recorded” is not the same as alive.",
        null,
        numbers([none, "No death recorded"], [recorded, "Death recorded"]),
      ),
    );

    const lands = stats.countries(entries);
    const top = lands[0];
    if (top) {
      figures.push(
        figure(
          "Where they are from",
          `${top.count} ${top.count === 1 ? "is" : "are"} from ${countryArticle(top.label)}${top.label}. Someone with two countries counts under both.`,
          (width) => [barChart(lands, width)],
          numbersList(lands),
        ),
      );
    }
  }

  figures.push(
    figure(
      "How you got there",
      "Guesses that did not count, typos the game fixed for you, and names matched through an alias.",
      null,
      numbers(
        [game.rejected, "Guesses rejected"],
        [entries.filter((entry) => entry.corrected).length, "Spellings corrected"],
        [entries.filter((entry) => entry.alias !== null).length, "Matched by alias"],
      ),
    ),
  );

  const supers = stats.superlatives(entries);
  const line = (label: string, who: string, fact: string) =>
    h("div", {}, h("dt", {}, label), h("dd", {}, h("strong", {}, who), fact ? ` · ${fact}` : ""));
  const languages = (entry: Entry) => `${entry.sitelinks} Wikipedia language${entry.sitelinks === 1 ? "" : "s"}`;
  const superlatives = [
    supers.mostFamous && line("Most famous", supers.mostFamous.name, languages(supers.mostFamous)),
    supers.mostObscure && count > 1 && line("Most obscure", supers.mostObscure.name, languages(supers.mostObscure)),
    supers.earliestBorn && line("Earliest born", supers.earliestBorn.name, stats.formatYear(supers.earliestBorn.birth!)),
    supers.latestBorn &&
      supers.latestBorn !== supers.earliestBorn &&
      line("Most recently born", supers.latestBorn.name, stats.formatYear(supers.latestBorn.birth!)),
    supers.longestGap &&
      line("Longest gap", stats.formatDuration(supers.longestGap.ms), `before ${supers.longestGap.after.name}`),
    supers.fastestTen &&
      line(
        "Fastest ten in a row",
        stats.formatDuration(supers.fastestTen.ms),
        `${supers.fastestTen.first.name} to ${supers.fastestTen.last.name}`,
      ),
  ].filter((node): node is HTMLDivElement => Boolean(node));

  const copy = h("button", { type: "button", class: "button" }, "Copy result");
  copy.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(resultLine(game));
      copy.textContent = "Copied";
    } catch {
      copy.textContent = "Couldn't copy";
    }
    window.setTimeout(() => (copy.textContent = "Copy result"), 1800);
  });

  let record = "";
  if (complete && best !== null) {
    record = best >= game.elapsedMs ? "Your best time for this game." : `Your best is ${formatTime(best)}.`;
  }
  const sentence = gameText(categories);

  const element = h(
    "section",
    { class: "results", "aria-label": "Result" },
    h(
      "header",
      { class: "result-head" },
      h("p", { class: "kicker" }, complete ? "Finished" : `Stopped short of ${TARGET}`),
      h("h2", {}, h("span", { class: "big" }, `${count} ${count === 1 ? people.replace(/men$/, "man").replace(/people$/, "person") : people}`), ` in ${formatTime(game.elapsedMs)}`),
      sentence !== people || record ? h("p", { class: "result-sub" }, [sentence !== people ? `Name ${TARGET} ${sentence}.` : "", record].filter(Boolean).join(" ")) : null,
      h(
        "div",
        { class: "actions" },
        copy,
        h("button", { type: "button", class: "button quiet", onclick: handlers.onNewGame }, "New game"),
      ),
    ),
    h("div", { class: "figures" }, ...figures.map((made) => made.node)),
    superlatives.length > 0
      ? h("section", { class: "block" }, h("h3", {}, "Superlatives"), h("dl", { class: "facts" }, ...superlatives))
      : null,
    count > 0
      ? h(
          "section",
          { class: "block" },
          h("h3", {}, "Everyone you named"),
          h("ol", { class: "names full" }, ...entries.map((entry, position) => entryItem(entry, position + 1, true))),
        )
      : null,
  );

  let drawn = 0;
  const layout = () => {
    for (const made of figures) {
      if (!made.chart || !made.draw) continue;
      const width = Math.floor(made.node.clientWidth);
      if (width === 0 || (width === drawn && made.chart.childElementCount > 0)) continue;
      made.chart.replaceChildren(...made.draw(width));
    }
    drawn = Math.floor(figures[0]?.node.clientWidth ?? 0);
  };
  return { element, layout };
}
