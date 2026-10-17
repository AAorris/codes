# functions/route.f
# maps method + path to a function name; first match wins
# Exact routes usually precede broader routes. To add nested meditation pages,
# place the following after the index route (and add meditation-child.f):
# route "GET" "/meditation/*" "meditation-child"
# after the index route. The slash keeps it from matching "/meditations".
route "GET" "/" "default"
route "GET" "/meditation" "meditation"
route "GET" "/northwestern-gate" "northwestern-gate"
route "GET" "/systems" "systems"
