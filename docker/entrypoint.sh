#!/bin/sh
set -eu

CONF_DIR="${RUSTNPS_CONF_DIR:-/etc/rustnps}"

if [ "$#" -eq 0 ]; then
    set -- rnps
fi

case "$1" in
    rnps)
        shift
        if [ "$#" -eq 0 ]; then
            set -- -conf_path="${CONF_DIR}/nps.conf"
        else
            for arg in "$@"; do
                case "$arg" in
                    -conf_path=*|--conf-path=*|-config=*|--config=*)
                        exec /usr/local/bin/rnps "$@"
                        ;;
                esac
            done
            set -- -conf_path="${CONF_DIR}/nps.conf" "$@"
        fi
        exec /usr/local/bin/rnps "$@"
        ;;
    rnpc)
        shift
        if [ "$#" -eq 0 ]; then
            set -- -config="${CONF_DIR}/npc.conf"
        fi
        exec /usr/local/bin/rnpc "$@"
        ;;
    -*)
        for arg in "$@"; do
            case "$arg" in
                -conf_path=*|--conf-path=*|-config=*|--config=*)
                    exec /usr/local/bin/rnps "$@"
                    ;;
            esac
        done
        exec /usr/local/bin/rnps -conf_path="${CONF_DIR}/nps.conf" "$@"
        ;;
    *)
        exec "$@"
        ;;
esac