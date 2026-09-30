import fs from 'node:fs/promises';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const SUPPORTED = new Set(['equals', 'contains', 'icontains', 'contains-any', 'contains-all', 'starts-with', 'regex', 'is-json', 'javascript', 'llm-rubric', 'assert-set']);
const TOP_LEVEL = new Set(['description', 'prompts', 'providers', 'tests', 'defaultTest', 'env', 'sharing', 'writeLatestResults', 'outputPath']);

function assertionsSupported(assertions, mode) {
  for (const a of assertions ?? []) {
    const base = a.type?.replace(/^not-/, '');
    if (!SUPPORTED.has(base)) throw new Error(`Unsupported assertion: ${a.type}. Use the Python wrapper for a custom grader.`);
    if (base === 'llm-rubric' && mode !== 'llm') throw new Error('llm-rubric requires mode=llm');
    if (typeof a.value === 'string' && a.value.startsWith('file://') && /\.py(?::|$)/.test(a.value)) {
      throw new Error('Promptfoo hides some Python execution errors as rejections. Use the native EvalProof Python adapter.');
    }
    if (base === 'assert-set') assertionsSupported(a.assert, mode);
  }
}

export function executionError(result) {
  if (!result) return false;
  if (result.error || result.metadata?.graderError) return true;
  // These are explicit execution-error sentinels in Promptfoo 0.123.0.
  if (/^(Custom function threw error:|Invalid regex pattern:|Scoring function error:)/.test(result.reason ?? '')) return true;
  return result.componentResults?.some(executionError) ?? false;
}

export async function createGrader(configPath, mode = 'deterministic') {
  process.env.PROMPTFOO_DISABLE_TELEMETRY = '1';
  process.env.PROMPTFOO_DISABLE_UPDATE_CHECK = '1';
  process.env.LOG_LEVEL = 'silent';
  const entry = import.meta.resolve('promptfoo');
  const require = createRequire(entry);
  const pkg = JSON.parse(await fs.readFile(path.resolve(path.dirname(fileURLToPath(entry)), '../../package.json'), 'utf8'));
  if (pkg.version !== '0.123.0') throw new Error(`Promptfoo ${pkg.version} has not passed conformance testing; install 0.123.0`);
  const absolute = path.resolve(configPath);
  const source = await fs.readFile(absolute, 'utf8');
  if (Buffer.byteLength(source) > 4 * 1024 * 1024) throw new Error('Config exceeds 4 MiB');
  const config = absolute.endsWith('.json') ? JSON.parse(source) : require('js-yaml').load(source);
  if (!config || typeof config !== 'object') throw new Error('Expected a Promptfoo configuration object');
  for (const key of Object.keys(config)) if (!TOP_LEVEL.has(key)) throw new Error(`Unsupported config field: ${key}`);
  if (!Array.isArray(config.tests) || !config.tests.length || config.tests.some(t => !t || typeof t !== 'object')) {
    throw new Error('v1 requires inline test records; materialize external datasets first');
  }
  for (const test of [config.defaultTest ?? {}, ...config.tests]) {
    const allowed = new Set(['description', 'vars', 'assert', 'options', 'threshold', 'metadata', 'providerOutput']);
    for (const key of Object.keys(test)) if (!allowed.has(key)) throw new Error(`Unsupported test field: ${key}`);
    for (const key of Object.keys(test.options ?? {})) {
      if (!['transform', 'provider', 'rubricPrompt'].includes(key)) throw new Error(`Unsupported test option: ${key}`);
    }
    if (Object.values(test.vars ?? {}).some(Array.isArray)) throw new Error('Materialize variable expansion into atomic tests first');
    assertionsSupported(test.assert, mode);
  }
  const p = await import('promptfoo');
  p.cache.disableCache();
  process.chdir(path.dirname(absolute));
  return async request => {
    const index = request.context?.test_index;
    if (!Number.isInteger(index) || index < 0 || index >= config.tests.length) throw new Error('Invalid context.test_index');
    const selected = config.tests[index];
    if (!(selected.assert?.length || config.defaultTest?.assert?.length)) throw new Error('Selected test has no assertions');
    const prompts = typeof config.prompts === 'string' ? [config.prompts] : config.prompts;
    const pi = request.context?.prompt_index ?? 0;
    if (!Array.isArray(prompts) || !Number.isInteger(pi) || pi < 0 || pi >= prompts.length) throw new Error('Invalid prompt selection');
    const providers = Array.isArray(config.providers) ? config.providers : [config.providers];
    const providerIndex = request.context?.provider_index ?? 0;
    if (!Number.isInteger(providerIndex) || providerIndex < 0 || providerIndex >= providers.length) throw new Error('Invalid provider selection');
    const originalProvider = providers[providerIndex];
    const providerId = typeof originalProvider === 'string' ? originalProvider : originalProvider?.id;
    if (typeof providerId !== 'string') throw new Error('Provider requires an explicit ID');
    if (originalProvider?.transform) throw new Error('Move provider transforms to test.options.transform before importing');
    const format = request.context?.output_format ?? 'text';
    if (!['text', 'json'].includes(format)) throw new Error('output_format must be text or json');
    const output = format === 'json' ? request.output : request.output_text;
    // A frozen provider fallback also handles falsey providerOutput without generating.
    const frozen = { id: () => providerId, config: originalProvider?.config ?? {}, callApi: async () => ({ output, cached: false, cost: 0 }) };
    const evaluation = await p.evaluate({
      ...config, prompts: [prompts[pi]], providers: [frozen],
      tests: [{ ...selected, providerOutput: output }],
      sharing: false, writeLatestResults: false, outputPath: undefined,
    }, { cache: false, maxConcurrency: 1, repeat: 1 });
    const summary = await evaluation.toEvaluateSummary();
    if (summary.results.length !== 1) throw new Error('A grading request expanded into multiple results');
    const row = summary.results[0];
    if (row.failureReason === p.ResultFailureReason.ERROR || executionError(row.gradingResult) || row.response?.error) {
      return { verdict: 'error', reason: 'Promptfoo reported a grader execution error' };
    }
    if (!row.gradingResult || typeof row.success !== 'boolean') throw new Error('Missing grading result');
    const result = { verdict: row.success ? 'accept' : 'reject', score: row.score, reason: String(row.gradingResult.reason ?? '').slice(0, 4000) };
    // row.cost measures generation, not necessarily all grading calls. Never label it
    // as actual LLM grading spend. The engine retains the declared callback reservation.
    if (mode === 'deterministic') result.cost_microusd = 0;
    return result;
  };
}

async function main() {
  const write = process.stdout.write.bind(process.stdout);
  process.stdout.write = () => true;
  process.stderr.write = () => true;
  const grade = await createGrader(process.argv[2], process.env.EVALPROOF_MODE);
  write('{"version":1,"ready":true,"adapter":"promptfoo","version_target":"0.123.0"}\n');
  let buffer = Buffer.alloc(0);
  for await (const chunk of process.stdin) {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.includes(10)) {
      const end = buffer.indexOf(10);
      if (end > 1024 * 1024) throw new Error('Request too large');
      const request = JSON.parse(buffer.subarray(0, end).toString('utf8'));
      buffer = buffer.subarray(end + 1);
      if (request.version !== 1 || !Number.isSafeInteger(request.id) || request.id < 0) throw new Error('Invalid protocol');
      let result;
      try { result = await grade(request); }
      catch { result = { verdict: 'error', reason: 'Promptfoo grading failed or used an unsupported configuration' }; }
      write(JSON.stringify({ version: 1, id: request.id, result }) + '\n');
    }
    if (buffer.length > 1024 * 1024) throw new Error('Request too large');
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const writeError = process.stdout.write.bind(process.stdout);
  main().catch(error => {
    writeError(JSON.stringify({version: 1, ready: false, reason: String(error.message ?? 'Adapter initialization failed').slice(0, 512)}) + '\n');
    process.exitCode = 2;
  });
}
