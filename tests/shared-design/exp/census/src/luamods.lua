-- census: load each lua test module and call its functions; run with cwd = a copy of lua-5.4.7-tests.
-- Lua caches a library's handle on first load, so a library meant to be global ("*") is loaded with "*" first.
package.cpath = "libs/?.so"
local function check(name, cond) print((cond and "OK " or "FAIL ") .. name) end
check("lib1 global load", package.loadlib("libs/lib1.so", "*") ~= nil)
local f = package.loadlib("libs/lib1.so", "onefunction")
local a, b = f(15, 25); check("lib1 onefunction", a == 25 and b == 15)
f = package.loadlib("libs/lib1.so", "anotherfunc"); check("lib1 anotherfunc", f(10, 20) == "10%20\n")
f = package.loadlib("libs/lib11.so", "luaopen_lib11"); check("lib11 -> lib1_export (undefined in lib11, found in lib1 at load)", f ~= nil and f() == "exported")
check("lib2 global load", package.loadlib("libs/lib2.so", "*") ~= nil)
local l2 = require "lib2"; check("lib2 id", select('#', l2.id(1, 2, 3)) == 3 and x == "lib2")
local v2 = require "lib2-v2"; check("lib2-v2 id (a second luaopen_lib2, local)", v2.id("x") == true and x == "lib2-v2")
local ok, l21 = pcall(require, "lib21"); check("lib21 -> luaopen_lib2 (undefined in lib21, found in the global lib2, not lib2-v2)", ok and select('#', l21.id(7, 8)) == 2 and l21.id(7) == 7)
local s = require "lib1.sub"; check("lib1.sub id", s.id(45) == 45)
