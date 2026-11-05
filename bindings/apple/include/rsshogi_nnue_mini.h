#ifndef RSSHOGI_NNUE_MINI_H
#define RSSHOGI_NNUE_MINI_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* ABI 1. All pointers must originate from this library instance.
 * Requests/model loads are serialized per handle. stop may run concurrently.
 * free must run only after ALL operations (including stop) have returned;
 * no further operation may begin. A freed handle must never be reused.
 * All char* results are owned JSON {ok:true,...} or {ok:false,error:"..."}.
 * Release those results with mini_string_free, never the platform's free().
 * Model input is borrowed only for the duration of load_model.
 * Request input is borrowed NUL-terminated UTF-8 JSON.
 */
typedef struct MiniEngine MiniEngine;
MiniEngine *mini_engine_new(void);
void mini_engine_free(MiniEngine *engine);
char *mini_engine_load_model(MiniEngine *engine, const uint8_t *bytes, size_t len);
char *mini_engine_request(MiniEngine *engine, const char *request_json);
/* Idle stop cancels the next search as well; completing that search consumes it. */
void mini_engine_stop(MiniEngine *engine);
void mini_string_free(char *json);
/* Transfer buffers for WebAssembly. Deallocate with the exact original length. */
uint8_t *mini_alloc(size_t len);
void mini_dealloc(uint8_t *ptr, size_t len);

#ifdef __cplusplus
}
#endif
#endif
