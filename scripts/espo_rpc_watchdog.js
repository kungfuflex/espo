#!/usr/bin/env node

'use strict';

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const DEFAULT_LOG_FILE = path.resolve(process.cwd(), 'logs', 'espo-rpc-watchdog.log');

const config = {
  rpcUrl: process.env.ESPO_WATCHDOG_RPC_URL || 'https://api.alkanode.com/rpc',
  rpcMethod: process.env.ESPO_WATCHDOG_RPC_METHOD || 'get_espo_height',
  rpcParams: parseJsonEnv('ESPO_WATCHDOG_RPC_PARAMS'),
  intervalMs: parsePositiveInteger('ESPO_WATCHDOG_INTERVAL_MS', 10_000),
  requestTimeoutMs: parsePositiveInteger('ESPO_WATCHDOG_REQUEST_TIMEOUT_MS', 8_000),
  tmuxCommandTimeoutMs: parsePositiveInteger('ESPO_WATCHDOG_TMUX_COMMAND_TIMEOUT_MS', 5_000),
  maxConsecutiveFailures: parsePositiveInteger('ESPO_WATCHDOG_MAX_FAILURES', 3),
  tmuxSession: process.env.ESPO_WATCHDOG_TMUX_SESSION || 'espo',
  runDir: process.env.ESPO_WATCHDOG_RUN_DIR || '~/espo',
  runCommand: process.env.ESPO_WATCHDOG_RUN_COMMAND || '~/espo/run.sh',
  warmupMs: parsePositiveInteger('ESPO_WATCHDOG_WARMUP_MS', 60_000),
  nofileLimit: parsePositiveInteger('ESPO_WATCHDOG_NOFILE_LIMIT', 100_000),
  logFile: process.env.ESPO_WATCHDOG_LOG_FILE || DEFAULT_LOG_FILE,
};

let consecutiveFailures = 0;
let totalChecks = 0;
let totalRestarts = 0;
let checkInFlight = false;
let shuttingDown = false;
let logStream = null;
let nextCheckAt = 0;

main();

function main() {
  setupLogging();

  log('info', 'watchdog starting', {
    pid: process.pid,
    node: process.version,
    platform: `${os.platform()} ${os.release()}`,
    config: redactedConfig(),
  });

  process.on('SIGINT', () => shutdown('SIGINT'));
  process.on('SIGTERM', () => shutdown('SIGTERM'));
  process.on('uncaughtException', (error) => {
    log('fatal', 'uncaught exception', serializeError(error));
    shutdown('uncaughtException', 1);
  });
  process.on('unhandledRejection', (reason) => {
    log('fatal', 'unhandled rejection', serializeError(reason));
    shutdown('unhandledRejection', 1);
  });

  runCheck();
  setInterval(() => {
    runCheck();
  }, config.intervalMs);
}

async function runCheck() {
  if (shuttingDown) {
    return;
  }

  const now = Date.now();
  if (now < nextCheckAt) {
    log('info', 'warmup in progress; skipping health check', {
      nextCheckAt: new Date(nextCheckAt).toISOString(),
      remainingMs: nextCheckAt - now,
    });
    return;
  }

  if (checkInFlight) {
    log('warn', 'previous health check still running; skipping this interval', {
      intervalMs: config.intervalMs,
    });
    return;
  }

  checkInFlight = true;
  const checkNumber = ++totalChecks;
  const startedAt = Date.now();

  log('info', 'health check started', {
    checkNumber,
    rpcUrl: config.rpcUrl,
    rpcMethod: config.rpcMethod,
    timeoutMs: config.requestTimeoutMs,
    consecutiveFailures,
  });

  try {
    const result = await callRpc(checkNumber);
    consecutiveFailures = 0;

    log('info', 'health check succeeded', {
      checkNumber,
      durationMs: Date.now() - startedAt,
      httpStatus: result.httpStatus,
      responseId: result.body.id,
      result: summarizeRpcResult(result.body.result),
      consecutiveFailures,
    });
  } catch (error) {
    consecutiveFailures += 1;

    log('error', 'health check failed', {
      checkNumber,
      durationMs: Date.now() - startedAt,
      consecutiveFailures,
      maxConsecutiveFailures: config.maxConsecutiveFailures,
      error: serializeError(error),
    });

    if (consecutiveFailures >= config.maxConsecutiveFailures) {
      log('error', 'failure threshold reached; restarting tmux service', {
        consecutiveFailures,
        maxConsecutiveFailures: config.maxConsecutiveFailures,
        tmuxSession: config.tmuxSession,
      });

      try {
        const restarted = await restartTmuxService();
        if (restarted) {
          totalRestarts += 1;
          consecutiveFailures = 0;
          nextCheckAt = Date.now() + config.warmupMs;
          log('info', 'restart sequence completed; failure counter reset', {
            totalRestarts,
            consecutiveFailures,
            warmupMs: config.warmupMs,
            nextCheckAt: new Date(nextCheckAt).toISOString(),
          });
        } else {
          log('error', 'restart sequence did not complete; watchdog will retry on next interval', {
            consecutiveFailures,
            nextRetryInMs: config.intervalMs,
          });
        }
      } catch (restartError) {
        log('error', 'restart sequence threw unexpectedly; watchdog will continue', {
          consecutiveFailures,
          nextRetryInMs: config.intervalMs,
          error: serializeError(restartError),
        });
      }
    }
  } finally {
    checkInFlight = false;
  }
}

async function callRpc(checkNumber) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), config.requestTimeoutMs);
  const requestBody = {
    jsonrpc: '2.0',
    id: `espo-watchdog-${process.pid}-${checkNumber}-${Date.now()}`,
    method: config.rpcMethod,
  };

  if (config.rpcParams !== undefined) {
    requestBody.params = config.rpcParams;
  }

  log('debug', 'sending rpc request', {
    checkNumber,
    body: requestBody,
  });

  try {
    const response = await fetch(config.rpcUrl, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'user-agent': `espo-rpc-watchdog/${process.pid}`,
      },
      body: JSON.stringify(requestBody),
      signal: controller.signal,
    });

    const responseText = await response.text();
    const parsedBody = parseRpcResponse(responseText, {
      httpStatus: response.status,
      ok: response.ok,
    });

    log('debug', 'received rpc response', {
      checkNumber,
      httpStatus: response.status,
      ok: response.ok,
      body: parsedBody || undefined,
      bodyPreview: parsedBody ? undefined : responseText.slice(0, 500),
      rawBytes: Buffer.byteLength(responseText),
    });

    if (!response.ok) {
      const detail = parsedBody ? JSON.stringify(parsedBody) : responseText.slice(0, 500);
      throw new Error(`RPC HTTP status ${response.status}: ${detail}`);
    }

    if (parsedBody.error) {
      throw new Error(`RPC error ${JSON.stringify(parsedBody.error)}`);
    }

    if (!Object.prototype.hasOwnProperty.call(parsedBody, 'result')) {
      throw new Error('RPC response did not include a result');
    }

    return {
      httpStatus: response.status,
      body: parsedBody,
    };
  } catch (error) {
    if (error && error.name === 'AbortError') {
      throw new Error(`RPC request timed out after ${config.requestTimeoutMs}ms`);
    }
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

async function restartTmuxService() {
  log('info', 'checking tmux session before restart', {
    tmuxSession: config.tmuxSession,
  });

  const hasSession = await commandSucceeds('tmux', ['has-session', '-t', config.tmuxSession]);
  log('info', 'tmux session status', {
    tmuxSession: config.tmuxSession,
    exists: hasSession,
  });

  if (hasSession) {
    const killResult = await runCommand('tmux', ['kill-session', '-t', config.tmuxSession], {
      action: 'kill existing tmux session',
    });
    if (!killResult.ok) {
      log('error', 'failed to kill existing tmux session; continuing with replacement attempt', {
        tmuxSession: config.tmuxSession,
        result: killResult,
      });
    }
  } else {
    log('warn', 'tmux session did not exist; continuing with new session creation', {
      tmuxSession: config.tmuxSession,
    });
  }

  const tmuxCommand = buildTmuxStartCommand();
  const createResult = await runCommand(
    'tmux',
    ['new-session', '-d', '-s', config.tmuxSession, tmuxCommand],
    {
      action: 'create replacement tmux session',
      commandInsideTmux: tmuxCommand,
    },
  );

  if (!createResult.ok) {
    log('error', 'failed to create replacement tmux session', {
      tmuxSession: config.tmuxSession,
      result: createResult,
    });
    return false;
  }

  const restartedSessionExists = await commandSucceeds('tmux', [
    'has-session',
    '-t',
    config.tmuxSession,
  ]);

  if (!restartedSessionExists) {
    log('error', 'tmux session disappeared immediately after creation', {
      tmuxSession: config.tmuxSession,
      detail:
        'new-session returned success, but has-session failed right after it. Check tmux output or run the configured run command manually.',
    });
    return false;
  }

  log('info', 'tmux session is running after restart', {
    tmuxSession: config.tmuxSession,
    warmupMs: config.warmupMs,
  });
  return true;
}

function buildTmuxStartCommand() {
  return [
    `cd ${shellQuote(config.runDir)}`,
    `ulimit -n ${config.nofileLimit}`,
    config.runCommand,
  ].join(' && ');
}

function commandSucceeds(command, args) {
  return new Promise((resolve) => {
    let settled = false;
    const child = spawn(command, args, { stdio: 'ignore' });
    const timeout = setTimeout(() => {
      if (settled) {
        return;
      }
      settled = true;
      child.kill('SIGTERM');
      log('error', 'command timed out', {
        command,
        args,
        timeoutMs: config.tmuxCommandTimeoutMs,
      });
      resolve(false);
    }, config.tmuxCommandTimeoutMs);

    child.on('error', (error) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timeout);
      log('error', 'command failed to start', {
        command,
        args,
        error: serializeError(error),
      });
      resolve(false);
    });
    child.on('close', (code, signal) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timeout);
      resolve(code === 0 && signal === null);
    });
  });
}

function runCommand(command, args, metadata = {}) {
  return new Promise((resolve) => {
    const startedAt = Date.now();
    log('info', 'running command', {
      ...metadata,
      command,
      args,
      timeoutMs: config.tmuxCommandTimeoutMs,
    });

    let settled = false;
    let stdout = '';
    let stderr = '';
    const child = spawn(command, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    const timeout = setTimeout(() => {
      if (settled) {
        return;
      }
      settled = true;
      child.kill('SIGTERM');
      const result = {
        ok: false,
        timedOut: true,
        ...metadata,
        command,
        args,
        durationMs: Date.now() - startedAt,
        timeoutMs: config.tmuxCommandTimeoutMs,
        stdout: stdout.trim() || undefined,
        stderr: stderr.trim() || undefined,
      };
      log('error', 'command timed out', result);
      resolve(result);
    }, config.tmuxCommandTimeoutMs);

    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString();
    });
    child.on('error', (error) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timeout);
      const result = {
        ok: false,
        ...metadata,
        command,
        args,
        durationMs: Date.now() - startedAt,
        error: serializeError(error),
        stdout: stdout.trim() || undefined,
        stderr: stderr.trim() || undefined,
      };
      log('error', 'command failed to start', result);
      resolve(result);
    });
    child.on('close', (code, signal) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timeout);
      const result = {
        ok: code === 0 && signal === null,
        ...metadata,
        command,
        args,
        code,
        signal,
        durationMs: Date.now() - startedAt,
        stdout: stdout.trim() || undefined,
        stderr: stderr.trim() || undefined,
      };

      if (result.ok) {
        log('info', 'command completed successfully', result);
      } else {
        log('error', 'command failed', result);
      }
      resolve(result);
    });
  });
}

function parseRpcResponse(responseText, responseMeta = {}) {
  try {
    return JSON.parse(responseText);
  } catch (error) {
    if (responseMeta.ok === false) {
      return null;
    }
    const preview = responseText.slice(0, 500);
    throw new Error(`RPC response was not valid JSON: ${preview}`);
  }
}

function summarizeRpcResult(result) {
  if (result === null || result === undefined) {
    return result;
  }

  if (typeof result !== 'object') {
    return result;
  }

  if (Object.prototype.hasOwnProperty.call(result, 'height')) {
    return { height: result.height };
  }

  const json = JSON.stringify(result);
  if (json.length <= 500) {
    return result;
  }

  return {
    type: Array.isArray(result) ? 'array' : 'object',
    keys: Array.isArray(result) ? undefined : Object.keys(result).slice(0, 20),
    preview: `${json.slice(0, 500)}...`,
  };
}

function parseJsonEnv(name) {
  const value = process.env[name];
  if (value === undefined || value.trim() === '') {
    return undefined;
  }

  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${name} must be valid JSON when set: ${error.message}`);
  }
}

function parsePositiveInteger(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === '') {
    return fallback;
  }

  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive integer`);
  }

  return parsed;
}

function setupLogging() {
  const logDir = path.dirname(config.logFile);
  fs.mkdirSync(logDir, { recursive: true });
  logStream = fs.createWriteStream(config.logFile, { flags: 'a' });
}

function log(level, message, data = {}) {
  const entry = {
    ts: new Date().toISOString(),
    level,
    message,
    ...data,
  };
  const line = JSON.stringify(entry);

  if (level === 'error' || level === 'fatal') {
    console.error(line);
  } else {
    console.log(line);
  }

  if (logStream) {
    logStream.write(`${line}\n`);
  }
}

function serializeError(error) {
  if (error instanceof Error) {
    return {
      name: error.name,
      message: error.message,
      stack: error.stack,
    };
  }

  return {
    value: error,
  };
}

function redactedConfig() {
  return {
    ...config,
    rpcParams: config.rpcParams === undefined ? undefined : '[configured]',
  };
}

function shutdown(reason, exitCode = 0) {
  if (shuttingDown) {
    return;
  }

  shuttingDown = true;
  log('info', 'watchdog shutting down', {
    reason,
    exitCode,
    totalChecks,
    totalRestarts,
    consecutiveFailures,
  });

  if (logStream) {
    logStream.end(() => process.exit(exitCode));
  } else {
    process.exit(exitCode);
  }
}

function shellQuote(value) {
  if (value.startsWith('~/')) {
    return `~/${shellQuote(value.slice(2))}`;
  }

  if (/^[A-Za-z0-9_./:-]+$/.test(value)) {
    return value;
  }

  return `'${value.replace(/'/g, `'\\''`)}'`;
}
