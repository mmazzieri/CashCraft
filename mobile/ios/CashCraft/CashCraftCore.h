// CashCraft's engine (crates/ffi): one call, JSON in and JSON out. See docs/mobile.md.
#pragma once

// Runs one request. The answer is never null and goes back to cashcraft_free once.
char *cashcraft_call(const char *request);

void cashcraft_free(char *answer);
