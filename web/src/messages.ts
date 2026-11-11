// The one-line outcome shown under the input. The wording table is in
// name100-frontend.md, "Rejection messages".

import type { LookupResponse, Match } from "./api";

/** Message text; `em` marks a name or the player's own words. */
export type Text = (string | { em: string })[];

export const COULD_NOT_CHECK: Text = ["Couldn't check that, try again."];
export const NOT_A_NAME: Text = ["That doesn't look like a name."];

export function alreadyNamed(name: string): Text {
  return ["Already named: ", { em: name }, "."];
}

function genderMessage(match: Match): Text {
  if (match.genders.length === 0) {
    return [{ em: match.name }, " doesn't count here: Wikidata doesn't record a gender for them."];
  }
  return [
    { em: match.name },
    " doesn't count here: Wikidata records their gender as ",
    { em: match.genders.join(" and ") },
    ".",
  ];
}

/** The message for a rejected lookup. */
export function rejection(response: LookupResponse): Text {
  const input = response.input.trim();
  const match = response.match;
  switch (response.reason) {
    case "ambiguous":
      return ["More than one person is called ", { em: input }, ". Be more specific."];
    case "not_a_name":
      return NOT_A_NAME;
    case "fictional":
      if (match) return [{ em: match.name }, " is fictional."];
      break;
    case "not_woman":
      if (match) return genderMessage(match);
      break;
    case "not_in_category": {
      if (!match) break;
      if (response.reason_detail?.startsWith("gender is")) return genderMessage(match);
      const what = match.description || match.categories[0];
      return what ? [{ em: match.name }, ` isn't one: ${what}.`] : [{ em: match.name }, " isn't one."];
    }
    default:
      break;
  }
  return ["Don't know ", { em: input }, "."];
}
