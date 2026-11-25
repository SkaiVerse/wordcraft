#ifndef WORDCRAFT_IOS_H
#define WORDCRAFT_IOS_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct WordCraftHost WordCraftHost;

// Lifecycle
WordCraftHost* wordcraft_create(float scale, float width, float height);
void wordcraft_destroy(WordCraftHost* host);

// Surface & Layout
void wordcraft_set_surface(WordCraftHost* host, void* metal_layer);
void wordcraft_resize(WordCraftHost* host, float width, float height, float scale);
void wordcraft_set_safe_insets(WordCraftHost* host, float top, float bottom, float left, float right, float keyboard_height);

// Input Events
void wordcraft_send_touch(WordCraftHost* host, int32_t phase, float x, float y, uint64_t touch_id, float force, int32_t pointer_type);
void wordcraft_send_mouse(WordCraftHost* host, float x, float y, int32_t button, bool is_down, float scroll_dx, float scroll_dy);
void wordcraft_send_key(WordCraftHost* host, int32_t key_code, uint32_t modifiers, bool is_down, const char* text);
void wordcraft_send_pinch_gesture(WordCraftHost* host, float scale, float velocity);
void wordcraft_send_pan_gesture(WordCraftHost* host, float dx, float dy);
void wordcraft_send_double_tap(WordCraftHost* host, float x, float y);
void wordcraft_send_three_finger_swipe(WordCraftHost* host, int32_t direction);
void wordcraft_send_context_menu(WordCraftHost* host, float x, float y);

// Dual-Mode State
int32_t wordcraft_get_input_mode(WordCraftHost* host);
void wordcraft_set_input_mode(WordCraftHost* host, int32_t mode);

// Keyboard Parity & UIKeyCommand Shortcut Discovery
char* wordcraft_get_key_commands_json(WordCraftHost* host);

// Apple Pencil & Text Insertion
int32_t wordcraft_insert_text_at_selection(WordCraftHost* host, const char* text);
void wordcraft_send_backspace(WordCraftHost* host);
int32_t wordcraft_send_pencil_stroke(WordCraftHost* host, int32_t page, const float* points, size_t count, const char* tool, const char* color, float width);

// Platform Action Pump, File Picker Completion, Pasteboard & Fonts
char* wordcraft_poll_platform_action(WordCraftHost* host);
void wordcraft_complete_file_pick(WordCraftHost* host, uint64_t pick_id, const char* paths_json);
void wordcraft_sync_pasteboard_text(WordCraftHost* host, const char* text);
int32_t wordcraft_register_font_path(WordCraftHost* host, const char* path);
int32_t wordcraft_register_font_bytes(WordCraftHost* host, const uint8_t* bytes, size_t len);
int32_t wordcraft_is_touch_drag_active(WordCraftHost* host);
int32_t wordcraft_is_point_in_canvas(WordCraftHost* host, float x, float y);

// Render Pass
int32_t wordcraft_render_frame(WordCraftHost* host);

// Command Dispatch (returns JSON allocated string, must free with wordcraft_free_string)
char* wordcraft_dispatch_command(WordCraftHost* host, const char* cmd, const char* params_json);
void wordcraft_free_string(char* s);

// Document I/O (Bytes & Paths for Security-Scoped UIDocumentPicker)
int32_t wordcraft_open_document_bytes(WordCraftHost* host, const char* name, const uint8_t* bytes, size_t len);
uint8_t* wordcraft_get_document_bytes(WordCraftHost* host, size_t* out_len);
void wordcraft_free_bytes(uint8_t* ptr, size_t len);
int32_t wordcraft_open_document_path(WordCraftHost* host, const char* path);
int32_t wordcraft_save_document_path(WordCraftHost* host, const char* path);
char* wordcraft_get_document_path(WordCraftHost* host);
int32_t wordcraft_is_document_dirty(WordCraftHost* host);

#ifdef __cplusplus
}
#endif

#endif // WORDCRAFT_IOS_H
