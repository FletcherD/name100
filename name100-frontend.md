# Name 100 Women — Web Game Spec (draft)

Nov 10, 2026 · draft for review

## Overview

A single-page browser game: type the names of 100 real, notable women as fast as you can. The browser runs the whole game. The lookup server (`name100.md`) is asked one question per guess, "is this a real, notable woman, and who?", and remembers nothing.

This is a draft. The decisions already made are listed first; the ones still open follow, each with a recommended default so the spec is buildable as it stands.

**Status:** a first version is built in `web/`, with open questions 1–6, 8 and 9 taken at their recommended defaults. Question 9's wording is implemented as recommended but still needs sign-off.

## Decided

| Topic | Decision |
| --- | --- |
| Where the game runs | In the browser. The server stays stateless: no sessions, no stored games. |
| Cheating | Not defended against. Anyone can script the API; the game is for fun. |
| Leaderboard | None on the server. |
| Timer | Counts up only. No countdown, no time limit. |
| Ambiguous names | The player is told the name is shared and asked to be more specific. No list of candidates is shown: picking from a list is too easy. |
| Accepted guesses | Shown with who the guess resolved to and their short description, for example `Lady Gaga [American singer-songwriter and actress]`, and the alias that matched when it was not the main name. |
| Hosting | The game is static files on GitHub Pages. The lookup server runs separately on a small host and allows the game's origin to call it. Development and testing stay on localhost until the game is finished. |
| Choosing the game | The player types a category into a search box ("actors") and picks from the matches; there is no fixed list of games. Up to two can be combined ("women" and "scientists"). A shared name resolves among the people in the category. |
| End of game | Statistical breakdowns of the women named: by occupation, by fame, and more. See "Result screen". |

## Open, with recommended defaults

| # | Question | Recommended default |
| --- | --- | --- |
| 1 | Can a player stop before 100? | Yes: a "Finish" button ends the game and shows the result for however many were named. |
| 2 | Is 100 the only target? | Yes for the first version. |
| 3 | Scoring | Time to 100 only. No points per name. |
| 4 | Sharing | A "Copy result" button that copies one line of text (count and time), no names. |
| 5 | Saved progress | The game in progress and the player's best time are kept in the browser's local storage. Nothing is sent anywhere. |
| 6 | What the page opens on | The original game, women, already selected, with the category search one click away. |
| 7 | Where the backend runs | A small always-on virtual server (2–4 GB of memory) with Caddy in front for HTTPS, or Fly.io to avoid server administration. Not needed until the game is ready to go public. |
| 8 | Tech | TypeScript and Vite, no UI framework. The page is one input, one list and a timer. |
| 9 | Wording of the "not a woman" rejection | See "Rejection messages". This is the most sensitive copy in the game and needs sign-off. |

## Choosing a category

Above the game sits a line stating what is being played: "Name 100 **women**". Clicking the category opens a search box.

- As the player types, the page asks `GET /v1/categories?q=<text>` (waiting 150 ms after the last keystroke) and lists up to ten matches. Each shows its name, what kind of thing it is, and its size: "actor · occupation · 412,000 people".
- Picking one makes it the game. A second can then be added the same way ("+ add another"), giving for example "Name 100 **women** who are **scientists**"; either can be removed. Two is the limit.
- When two are combined the page cannot know how many people are in both; it shows each one's count and nothing more. For the common case of `women` plus one other, the other's `women` count is the answer and is shown.
- A category with fewer than a few hundred people gets a note that 100 may be out of reach.
- The choice is kept in the page address (`?category=women,occupation:Q901`), so a game can be linked to, and in local storage with the rest of the game.
- Changing category during a game starts a new game, after a confirmation.

How the sentence is worded depends on the category's `kind`: "Name 100 actors" (occupation: the label, made plural), "Name 100 people from France" (citizenship), "Name 100 people in tennis" (sport), "Name 100 people who play guitar" (instrument), "Name 100 winners of the Nobel Prize in Physics" (award). The wording table lives in the game's code.

Every lookup then carries the choice: `GET /v1/lookup?q=<name>&category=<id>[,<id>]`. With `women` alone the parameter is left off, which plays the original game with its original rule for shared names.

## The game

**Start.** The page opens on an empty list, a counter at `0 / 100`, a timer at `0:00` and a focused text input. The timer starts on the player's first submitted guess, not on page load.

**Guessing.** The player types a name and presses Enter. The game sends one lookup per submitted guess (never per keystroke), clears the input straight away so the player can keep typing, and shows the outcome when the answer arrives. Guesses are answered in the order they were sent.

**Accepted.** The person is added to the top of the list and the counter goes up. Each entry shows:

- the person's main name (`match.name`)
- their description in brackets (`match.description`), when there is one
- when the guess matched an alias (`matched_as` is `alias`): "as *Stefani Germanotta*"
- when the guess was corrected (`corrected_from` is set): a small "spelling corrected" mark

**Duplicates.** The game keeps the QIDs it has accepted. If an accepted answer's `match.qid` is already in the list, it does not count, and the message says who it was already counted as: "Already named: Lady Gaga". This is why "Lady Gaga" and "Stefani Germanotta" score once.

**Finish.** At 100 accepted names, or when the player presses Finish, the timer stops, the input is disabled and the result screen replaces the input. Wikipedia links are not shown during play, to keep the player on the page.

**New game.** Clears the list and timer after a confirmation.

## Result screen

Shown when the game ends. Everything on it is computed in the browser from the answers already received; nothing more is fetched.

**Headline.** The count and the time, with the "Copy result" button.

**Breakdowns.** Each is one chart with a one-line caption stating its most striking fact ("41 of your 100 were musicians").

| Breakdown | Chart | Data |
| --- | --- | --- |
| What they do | Pie chart of main occupation category (left out when the game's category is itself an occupation) | `match.categories[0]`. Categories with fewer than 3 women are merged into "Other"; women with no category are "Uncategorized". |
| How famous | Histogram of Wikipedia article counts, in bands (under 10, 10–24, 25–49, 50–99, 100 and over) | `match.sitelinks` |
| When they lived | Histogram by birth decade from 1900, by century before that | `match.birth_year`; women with no recorded birth year are counted separately |
| Living or not | Two numbers | `match.death_year` is null or not. Wikidata does not always record deaths, so the caption says "no death recorded", not "alive". |
| Where they are from | Bar chart of the ten most common countries | `match.citizenships`; a woman with two countries counts under both, and the caption says so |
| How you got there | Three numbers: guesses rejected, spellings corrected, and names matched through an alias | counted during play |

**Superlatives.** One line each:

- most famous and most obscure woman named (highest and lowest `match.sitelinks`)
- earliest and most recent birth year
- longest gap between two accepted names, and the fastest ten in a row

**The list.** All accepted women in the order named, each with her description, the time she was named, and a link to Wikipedia (`match.wikipedia_url`).

**About the occupation categories.** The server reduces Wikidata's thousands of occupations to about two dozen categories (Musician, Actor, Athlete, Politician, Writer, Scientist or scholar, and so on). Most people have several occupations, and Wikidata does not say which is the main one, so the server picks: the one named first in the person's description, else royalty, else politician for an office holder, else whichever covers most of their occupations. It is a good guess, not a fact. `match.categories` lists the rest in order, so a later version could offer "count every category" as an alternative view. The pie chart uses only the first, because a pie's slices must add up to the whole.

**Charts.** Drawn as inline SVG by the game's own code; no charting library. Each chart has a text alternative listing its numbers.

## Rejection messages

One line under the input, replaced by the next guess's outcome. The player's text is not restored to the input.

| API `reason` | Message | Notes |
| --- | --- | --- |
| `not_found` | "Don't know *{input}*." | Also shown when the typo was too far off to correct. |
| `ambiguous` | "More than one person is called *{input}*. Be more specific." | `candidates` is ignored. |
| `not_a_name` | "That doesn't look like a name." | Empty input is not sent at all. |
| `fictional` | "*{match.name}* is fictional." | |
| `not_in_category` | "*{match.name}* isn't one: {what they are}." | *{what they are}* is `match.description`, falling back to `match.categories[0]`. When `reason_detail` starts with "gender is" (the `women` or `men` part failed), use the `not_woman` wording instead. |
| `not_woman` | "*{match.name}* doesn't count here: Wikidata records their gender as *{gender}*." | *{gender}* is `match.genders` joined with "and". When it is empty: "…Wikidata doesn't record a gender for them." |
| duplicate (client side) | "Already named: *{match.name}*." | |

The `not_woman` wording needs care. The data is Wikidata's, it is sometimes wrong or out of date, and for people such as Miley Cyrus (recorded as genderfluid) the message is about a real person's identity. Recommended: state it as what Wikidata records, never as a fact about the person, and say nothing more. For example: "Miley Cyrus doesn't count here: Wikidata records their gender as genderfluid." The sentence is built from `match.genders`, which lists the labels on their own (`["genderfluid"]`, `["non-binary", "trans man"]`, or empty when none is recorded).

## What the game uses from the API

Two endpoints: `GET /v1/categories?q=<text>` for the category search, reading `id`, `label`, `kind`, `people` and `women`; and `GET /v1/lookup?q=<name>[&category=...]`. Fields read from a lookup:

| Field | Used for |
| --- | --- |
| `verdict`, `reason` | accept or pick the rejection message |
| `match.qid` | duplicate detection |
| `match.name`, `match.description` | the list entry |
| `matched_name`, `matched_as` | "as *alias*" on the entry |
| `corrected_from` | the "spelling corrected" mark |
| `reason_detail` | telling a gender rejection from a category one |
| `match.wikipedia_url` | the list on the result screen |
| `match.sitelinks`, `match.birth_year`, `match.death_year` | result breakdowns |
| `match.categories`, `match.citizenships` | result breakdowns |
| `match.genders` | the `not_woman` message |

Not used: `candidates`, `normalized`, `latency_us`, `matched_via`, `match.occupations`.

**Failures.** If a lookup fails or takes more than 5 seconds, the guess is put back in the input with "Couldn't check that, try again." The timer keeps running. A 400 from the server is treated the same as `not_a_name`.

## State kept in the browser

Saved to local storage after every accepted guess, so a reload resumes the game:

- the accepted entries, in order: QID, name, description, matched alias, corrected flag, the time named, and the fields the result screen needs (Wikipedia URL, sitelinks, birth and death years, categories, citizenships)
- elapsed time and whether the timer is running
- counts of rejected and corrected guesses
- the best finished time for 100

Elapsed time is tracked as accumulated play time, so closing the tab pauses the clock. That is exploitable, which is fine given the decision on cheating.

## Layout and behaviour

- One column, usable on a phone: counter and timer at the top, the input beneath them, the message line, then the list with the newest entry first.
- The input keeps focus at all times during play. Autocorrect, autocapitalise and browser autocomplete are turned off on it, so the phone keyboard does not rewrite names.
- No suggestions or autocomplete of names, for the same reason ambiguous names show no list.
- Works with keyboard only. Outcome messages are announced to screen readers.
- Light and dark themes following the system setting.

## Lookup server: what the game relies on

All of this is built and tested:

| Feature | Detail |
| --- | --- |
| Which name matched | `matched_name` and `matched_as` in the response. |
| Category games | `/v1/categories` to search, and `category=` on a lookup. |
| Breakdown data | `match.categories`, `match.occupations`, `match.citizenships` and `match.genders`, as English labels. |
| Calls from another origin | The server answers cross-origin requests from the origins listed in `cors_origins`. Pages on `http://localhost:<port>` are allowed by default, which is all local testing needs. |
| Rate limit | 600 lookups a minute per address by default (`rate_limit_per_minute`), answered with 429 and a `Retry-After` header. The game treats a 429 like any other failed lookup. |

Still to do when the game goes public, none of it code:

- Set `cors_origins` to the GitHub Pages origin and turn `cors_allow_localhost` off.
- Run the server behind HTTPS (the page will be served over HTTPS, and browsers refuse to call plain HTTP from it).
- Set `trust_forwarded_for` if it sits behind a proxy, so the rate limit sees real client addresses.

## Not in the first version

- Accounts, server-side scores or leaderboards
- A choice of target other than 100
- Free-text categories ("90s British pop singers") and date ranges
- Multiplayer or head-to-head play
- Hints, suggestions or a candidate picker
- Translations of the interface

## To settle before building

- [ ] Open questions 1–9 above: accept the defaults or change them.
- [ ] Look through the catalogue on the full index: which categories are offered, whether junk classes got in, and whether awards and positions group sensibly ("Nobel Prize", "head of state").
- [ ] Check the occupation categories on the full index: how many women end up "Uncategorized", and whether any category is doing too much work.
- [ ] Sign off the `not_woman` wording.
- [ ] After the full index is built, play a few rounds against it and check how often real guesses come back `not_found` or `ambiguous`. If either is common, those two messages need more thought than one line each.
