(function (window, $) {
    if (!window || !$ || !window.localStorage) {
        return;
    }

    var PREFIX = 'rustnps.tableState.';

    function storageKey(key) {
        return PREFIX + key;
    }

    function readState(key) {
        try {
            var raw = window.localStorage.getItem(storageKey(key));
            return raw ? JSON.parse(raw) : null;
        } catch (e) {
            return null;
        }
    }

    function writeState(key, state) {
        try {
            window.localStorage.setItem(storageKey(key), JSON.stringify(state));
        } catch (e) {
        }
    }

    function wrapCallback(original, after) {
        if (!original) {
            return after;
        }
        return function () {
            var result = original.apply(this, arguments);
            after.apply(this, arguments);
            return result;
        };
    }

    function cloneColumns(columns) {
        return $.extend(true, [], columns || []);
    }

    function applyHiddenColumns(columns, hiddenColumns) {
        var hiddenMap = {};
        hiddenColumns.forEach(function (field) {
            hiddenMap[field] = true;
        });
        return columns.map(function (column) {
            if (Array.isArray(column)) {
                return applyHiddenColumns(column, hiddenColumns);
            }
            if (column && column.field && hiddenMap[column.field]) {
                var next = $.extend(true, {}, column);
                next.visible = false;
                return next;
            }
            return $.extend(true, {}, column);
        });
    }

    function snapshot($table) {
        var options = $table.bootstrapTable('getOptions');
        var visibleColumns = $table.bootstrapTable('getVisibleColumns');
        var visibleMap = {};
        visibleColumns.forEach(function (column) {
            if (column && column.field) {
                visibleMap[column.field] = true;
            }
        });

        var hiddenColumns = [];
        (options.columns || []).forEach(function (column) {
            if (Array.isArray(column)) {
                column.forEach(function (nested) {
                    if (nested && nested.field && !visibleMap[nested.field]) {
                        hiddenColumns.push(nested.field);
                    }
                });
                return;
            }
            if (column && column.field && !visibleMap[column.field]) {
                hiddenColumns.push(column.field);
            }
        });

        return {
            pageSize: options.pageSize,
            pageNumber: options.pageNumber,
            searchText: options.searchText || '',
            sortName: options.sortName || '',
            sortOrder: options.sortOrder || '',
            hiddenColumns: hiddenColumns
        };
    }

    function bootstrapOptions(key, options) {
        var state = readState(key) || {};
        var merged = $.extend(true, {}, options);

        if (state.pageSize) {
            merged.pageSize = state.pageSize;
        }
        if (state.pageNumber) {
            merged.pageNumber = state.pageNumber;
        }
        if (typeof state.searchText === 'string') {
            merged.searchText = state.searchText;
        }
        if (state.sortName) {
            merged.sortName = state.sortName;
        }
        if (state.sortOrder) {
            merged.sortOrder = state.sortOrder;
        }
        if (Array.isArray(state.hiddenColumns) && state.hiddenColumns.length > 0 && merged.columns) {
            merged.columns = applyHiddenColumns(cloneColumns(merged.columns), state.hiddenColumns);
        }

        var save = function () {
            var $table = $(this);
            if (!$table.length || !$table.data('bootstrap.table')) {
                return;
            }
            writeState(key, snapshot($table));
        };

        merged.onPageChange = wrapCallback(merged.onPageChange, save);
        merged.onSearch = wrapCallback(merged.onSearch, save);
        merged.onSort = wrapCallback(merged.onSort, save);
        merged.onColumnSwitch = wrapCallback(merged.onColumnSwitch, save);
        merged.onRefresh = wrapCallback(merged.onRefresh, save);
        merged.onLoadSuccess = wrapCallback(merged.onLoadSuccess, save);

        return merged;
    }

    window.npsTableState = {
        bootstrapOptions: bootstrapOptions,
        save: function (key, tableSelector) {
            var $table = $(tableSelector);
            if (!$table.length || !$table.data('bootstrap.table')) {
                return;
            }
            writeState(key, snapshot($table));
        },
        load: readState
    };
})(window, window.jQuery);