// Whether every key of a settings file is one the installed application declares, with a value
// its declaration takes, and one its code reads: `node declared_settings.js <application dir>
// <settings.json>` prints a line per key and leaves with 1 when any key fails.
//
// A key is looked for as it is written, and as the constant the bundle keeps it in
// (`nti="security.workspace.trust.enabled"`), because the application's code mostly reads a
// setting by such a name. A declaration is the object literal of the key in the configuration
// registry (`"update.mode":{type:"string",enum:[…]…}` or `[nti]:{type:"boolean",…}`); a read is
// `getValue(<key or constant>)`. A setting renamed in a later version fails all three.
const fs = require("fs"), path = require("path");
const [app, file] = process.argv.slice(2);
// The window's code and the main process's, where the updater reads its own setting.
const code = ["vs/workbench/workbench.desktop.main.js", "main.js"]
  .map((part) => fs.readFileSync(path.join(app, "resources/app/out", part), "utf8"))
  .join("\n");
// The object literal that starts at `at`, braces matched.
function literal(at) {
  const open = code.indexOf("{", at);
  let depth = 0;
  for (let i = open; i < code.length; i++) {
    if (code[i] === "{") depth++;
    else if (code[i] === "}" && --depth === 0) return code.slice(at, i + 1);
  }
  return null;
}
const settings = JSON.parse(fs.readFileSync(file, "utf8"));
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
let failed = 0;
for (const [key, value] of Object.entries(settings)) {
  const quoted = JSON.stringify(key);
  let names = [quoted];
  for (const m of code.matchAll(new RegExp("([A-Za-z_$][\\w$]*)=" + esc(quoted) + "[,;]", "g"))) names.push(m[1]);
  let declaration = null;
  for (const name of names) {
    const at = name === quoted ? code.indexOf(quoted + ":{type:") : code.indexOf("[" + name + "]:{type:");
    if (at >= 0) { declaration = literal(at); break; }
  }
  const read = names.some((name) => code.includes("getValue(" + name + ")") || code.includes("getValue(" + name + ","));
  const type = declaration && (declaration.match(/type:"(\w+)"/) || [])[1];
  const en = declaration && declaration.match(/enum:\[([^\]]*)\]/);
  const choices = en ? JSON.parse("[" + en[1] + "]") : null;
  const typeOk = type === (typeof value === "number" ? "number" : typeof value);
  const enumOk = !choices || choices.includes(value);
  const ok = declaration && read && typeOk && enumOk;
  if (!ok) failed++;
  console.log(`${ok ? "ok" : "FAIL"} ${key}=${JSON.stringify(value)} declared=${!!declaration} type=${type} enum=${JSON.stringify(choices)} read=${read} names=${names.join(" ")}`);
}
process.exit(failed ? 1 : 0);
