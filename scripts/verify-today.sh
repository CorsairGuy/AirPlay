#!/usr/bin/env bash
# Прогон проверок для ветки review/today-20261127.
#
#   ./scripts/verify-today.sh            # всё: линт, тесты, interop
#   ./scripts/verify-today.sh quick      # только линт и тесты
#   ./scripts/verify-today.sh interop    # только interop с pyatv
#   OFFLINE=1 ./scripts/verify-today.sh  # без сети (нужен прогретый кэш)
#
# Скрипт ничего не устанавливает и не трогает сеть, кроме запуска приёмника
# на loopback в разделе interop. Код возврата — код первой упавшей команды.
set -uo pipefail
cd "$(dirname "$0")/.."

mode="${1:-all}"
offline=()
if [ "${OFFLINE:-0}" = "1" ]; then
    offline=(--offline)
fi

failed=0
step() {
    echo
    echo "=== $*"
    "$@"
    local code=$?
    if [ "$code" -ne 0 ]; then
        echo "!!! FAILED ($code): $*"
        failed=1
    fi
    return "$code"
}

if [ "$mode" != "interop" ]; then
    step cargo fmt --all --check
    step cargo clippy --workspace --all-targets "${offline[@]}" -- -D warnings

    # Ядро сегодняшних изменений: часы, NQPTP SHM, эпоха RTP, вывод.
    step cargo test "${offline[@]}" -p clock -p timesync -p audio_stream -p media

    # Весь workspace: остальные крейты могли поехать от слияний.
    step cargo test "${offline[@]}" --workspace

    # Аудио-профиль без видео/зеркалирования — ровно то, что нужно iPhone.
    step cargo test "${offline[@]}" -p runtime --no-default-features

    # Зеркалирование не входит в проверку iPhone↔Windows, но не должно
    # ломаться от правок общего кода.
    step cargo test "${offline[@]}" -p runtime --features mirroring
fi

if [ "$mode" != "quick" ]; then
    echo
    echo "=== interop: независимый клиент pyatv против runtime-харнесса"
    if ! python3 -c "import pyatv, srptools" 2>/dev/null; then
        echo "SKIP: нет pyatv/srptools."
        echo '      pip install "pyatv==0.14.5" "srptools==1.0.1"'
    else
        step cargo build "${offline[@]}" -p runtime --example interop_receiver
        if [ "$failed" -eq 0 ]; then
            state="$(mktemp -d)"
            log="$(mktemp)"
            ./target/debug/examples/interop_receiver --port 7123 --pin 1234 \
                --state-dir "$state" > "$log" 2>&1 &
            pid=$!
            ready=0
            for _ in $(seq 1 100); do
                if grep -q "READY 7123" "$log"; then
                    ready=1
                    break
                fi
                sleep 0.1
            done
            if [ "$ready" -ne 1 ]; then
                echo "!!! FAILED: харнесс не поднялся"
                cat "$log"
                failed=1
            else
                step python3 tests/interop/pyatv_client.py 127.0.0.1 7123 1234
            fi
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
            echo "--- журнал харнесса (хвост) ---"
            tail -n 40 "$log"
            rm -rf "$state" "$log"
        fi
    fi
fi

echo
if [ "$failed" -ne 0 ]; then
    echo "ИТОГ: есть падения, см. '!!! FAILED' выше."
    exit 1
fi
echo "ИТОГ: все запущенные проверки прошли."
