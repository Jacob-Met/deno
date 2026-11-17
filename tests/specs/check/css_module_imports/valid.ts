import sheet from "./style.css" with { type: "css" };
const { default: dynamicSheet } = await import("./style.css", {
  with: { type: "css" },
});

const rules: readonly CSSRule[] = sheet.cssRules;
const dynamicRules: readonly CSSRule[] = dynamicSheet.cssRules;
console.log(sheet instanceof CSSStyleSheet, rules.length, dynamicRules.length);
