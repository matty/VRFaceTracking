#define _GNU_SOURCE
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#define MAP_BYTES 0xC4000u
#define COUNTER_OFFSET (800000u + 24u)
#define LOG_PATH "/data/local/tmp/questpro-live-v9.log"

static void *snapshot(void *unused) {
    (void)unused;
    FILE *maps = fopen("/proc/self/maps", "r");
    if (!maps) return NULL;
    volatile uint8_t *slots[16];
    size_t count = 0;
    char line[1024];
    while (fgets(line, sizeof(line), maps)) {
        unsigned long long start, end;
        if (sscanf(line, "%llx-%llx", &start, &end) == 2 &&
            end - start == MAP_BYTES && strstr(line, "/dmabuf:dmabuf") && count < 16)
            slots[count++] = (volatile uint8_t *)(uintptr_t)start;
    }
    fclose(maps);
    FILE *log = fopen(LOG_PATH, "a");
    if (!log) return NULL;
    fprintf(log, "DIAGNOSTIC_MAP_COUNT %zu\n", count);
    for (int pass = 0; pass < 2; ++pass) {
        sleep(2);
        for (size_t i = 0; i < count; ++i) {
            uint32_t counter;
            memcpy(&counter, (const void *)(slots[i] + COUNTER_OFFSET), sizeof(counter));
            unsigned nonzero = 0, samples = 0;
            for (unsigned y = 23; y < 400; y += 47)
                for (unsigned x = 819; x < 2000; x += 89) {
                    nonzero += slots[i][(size_t)y * 2000 + x] != 0;
                    ++samples;
                }
            fprintf(log, "DIAGNOSTIC_MAP pass=%d slot=%zu counter=%u face_nonzero=%u/%u\n",
                    pass, i, counter, nonzero, samples);
        }
        fflush(log);
    }
    fclose(log);
    return NULL;
}

__attribute__((constructor)) static void start_snapshot(void) {
    pthread_t thread;
    if (pthread_create(&thread, NULL, snapshot, NULL) == 0) pthread_detach(thread);
}
