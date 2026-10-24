const fs = require('node:fs');
const path = require('node:path');
const Ajv = require('ajv');
const addFormats = require('ajv-formats');
const schemas = require('@sketch-hq/sketch-file-format').default;
const root = path.resolve(__dirname, '..');
const entries = JSON.parse(fs.readFileSync(path.join(root, 'deliverables/native-document.json'), 'utf8'));
const ajv = new Ajv({ allErrors: true, strict: false, unicodeRegExp: false });
addFormats(ajv);
const validators = {
  document: ajv.compile(schemas.document),
  page: ajv.compile(schemas.page),
  meta: ajv.compile(schemas.meta),
  user: ajv.compile(schemas.user),
};
const result = { schemaPackage: '@sketch-hq/sketch-file-format@6.5.0', checks: [], structuralChecks: [], valid: true, targetEditorImportTested: false };
for (const [name, value] of Object.entries(entries)) {
  const kind = name.startsWith('pages/') ? 'page' : name.replace('.json', '');
  const validate = validators[kind];
  const valid = validate(value);
  const errors = valid ? [] : validate.errors.filter(e => !['oneOf','const','required','additionalProperties'].includes(e.keyword)).slice(0, 30);
  result.checks.push({name, valid, errors: valid ? [] : (errors.length ? errors : validate.errors.slice(0, 15))});
  if (!valid) result.valid = false;
}
const all = [];
const walk = o => { all.push(o); for (const c of o.layers || []) walk(c); };
for (const [name, value] of Object.entries(entries)) if (name.startsWith('pages/')) walk(value);
const artboards = all.filter(o => o._class === 'artboard');
const artboardIds = new Set(artboards.map(o => o.do_objectID));
const masters = new Set(all.filter(o => o._class === 'symbolMaster').map(o => o.symbolID));
const ids = all.map(o => o.do_objectID);
const flows = all.filter(o => o.flow);
const checks = [
  ['unique-layer-ids', new Set(ids).size === ids.length],
  ['prototype-destinations-exist', flows.every(o => artboardIds.has(o.flow.destinationArtboardID))],
  ['symbol-sources-exist', all.filter(o=>o._class==='symbolInstance').every(o=>masters.has(o.symbolID))],
  ['no-raster-only-screens', all.every(o=>o._class!=='bitmap')],
  ['text-is-editable', all.filter(o=>o._class==='text').every(o=>typeof o.attributedString.string==='string')],
  ['utf16-text-ranges', all.filter(o=>o._class==='text').every(o=>o.attributedString.attributes[0].length===o.attributedString.string.length)],
];
for (const [name, valid] of checks) { result.structuralChecks.push({name,valid}); if (!valid) result.valid=false; }
result.counts = { artboards: artboards.length, layers: all.length, textLayers: all.filter(o=>o._class==='text').length, prototypeLinks: flows.length, symbols: masters.size };
fs.writeFileSync(path.join(root, 'validation.json'), JSON.stringify(result, null, 2));
for (const c of result.checks.filter(c=>!c.valid)) console.error(c.name, JSON.stringify(c.errors, null, 2));
console.log(JSON.stringify({valid:result.valid,counts:result.counts, structuralChecks:result.structuralChecks},null,2));
process.exitCode=result.valid?0:1;
