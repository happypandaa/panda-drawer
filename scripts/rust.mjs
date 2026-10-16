import { run } from './process.mjs';

try {
  await run('cargo', process.argv.slice(2));
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
