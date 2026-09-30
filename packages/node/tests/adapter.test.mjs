import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { createGrader, executionError } from '../adapter.mjs';

const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'evalproof-node-'));
process.env.PROMPTFOO_CONFIG_DIR = path.join(directory, 'runtime');
const base = { prompts: ['Grade {{question}}'], providers: ['echo'], tests: [{ vars: { question: 'invoice' }, assert: [{ type: 'javascript', value: 'JSON.parse(output).total === 125' }] }] };
async function grader(config = base, mode = 'deterministic') {
  const file = path.join(directory, `${Math.random()}.json`);
  await fs.writeFile(file, JSON.stringify(config));
  return createGrader(file, mode);
}
function request(output) { return { version: 1, id: 1, output, output_text: JSON.stringify(output), context: { test_index: 0 } }; }

test('native assertion accepts correct output and rejects mutated amount', async () => {
  const grade = await grader();
  assert.equal((await grade(request({ total: 125 }))).verdict, 'accept');
  assert.equal((await grade(request({ total: 1250 }))).verdict, 'reject');
});
test('native thresholds and transforms retain their semantics', async () => {
  const config = { ...base, tests: [{ threshold: 0.7, options: { transform: 'JSON.parse(output).total' }, assert: [{ type: 'javascript', value: 'output === 125 ? 0.8 : 0.2', threshold: 0.5 }] }] };
  const grade = await grader(config);
  assert.equal((await grade(request({ total: 125 }))).verdict, 'accept');
  assert.equal((await grade(request({ total: 1250 }))).verdict, 'reject');
});
test('JavaScript exception cannot count as a rejection', async () => {
  const grade = await grader({ ...base, tests: [{ assert: [{ type: 'javascript', value: 'throw new Error("failed")' }] }] });
  assert.equal((await grade(request({ total: 125 }))).verdict, 'error');
});
test('default assertions participate in aggregation', async () => {
  const grade = await grader({ ...base, defaultTest: { assert: [{ type: 'contains', value: 'invoice_id' }] } });
  assert.equal((await grade(request({ total: 125 }))).verdict, 'reject');
  assert.equal((await grade(request({ total: 125, invoice_id: 'INV-42' }))).verdict, 'accept');
});
test('rejects unsupported and unbudgeted model assertions before grading', async () => {
  await assert.rejects(grader({ ...base, scenarios: [] }), /Unsupported config/);
  await assert.rejects(grader({ ...base, tests: [{ assert: [{ type: 'llm-rubric', value: 'correct' }] }] }), /mode=llm/);
});
test('nested model provider errors remain errors even if aggregation passes', () => {
  assert.equal(executionError({ pass: true, componentResults: [{ pass: false, metadata: { graderError: true } }] }), true);
});
test('falsey supplied outputs use the frozen provider, never generation', async () => {
  const grade = await grader({ ...base, tests: [{ assert: [{ type: 'equals', value: 'false' }] }] });
  assert.equal((await grade(request(false))).verdict, 'accept');
});
