import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import Ajv2020 from 'ajv/dist/2020.js';

const schema = JSON.parse(await readFile(new URL('../schemas/panda.schema.json', import.meta.url), 'utf8'));
const cases = JSON.parse(await readFile(new URL('../schemas/fixtures/panda-manifest-cases.json', import.meta.url), 'utf8'));
// Rust integer format annotations are editor hints; structural integer checks
// and the explicit protocol/version constants remain active.
const validate = new Ajv2020({ strict: false, validateFormats: false, allErrors: true }).compile(schema);
for (const fixture of cases) {
  test(`panda.json schema: ${fixture.name}`, () => {
    assert.equal(validate(fixture.manifest), fixture.valid, JSON.stringify(validate.errors));
  });
}
