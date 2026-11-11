# The tessera-capi C++ programs: what a C++ producer's build looks like. nanoarrow comes from its
# CMake package, tessera-capi from its pkg-config file. TESSERA_LINK picks the shared or the static
# library, so both link paths are exercised.
cmake_minimum_required(VERSION 3.20)
project(tessera_capi_cpp LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(TESSERA_LINK "shared" CACHE STRING "link tessera-capi as 'shared' or 'static'")

find_package(Threads REQUIRED)
find_package(nanoarrow CONFIG REQUIRED)
find_package(PkgConfig REQUIRED)
pkg_check_modules(TESSERA REQUIRED tessera-capi)

if(TESSERA_LINK STREQUAL "static")
  # The archive itself, then the system libraries it needs (Libs.private), without -ltessera_capi.
  list(REMOVE_ITEM TESSERA_STATIC_LIBRARIES tessera_capi)
  set(TESSERA_LINK_LIBS "${TESSERA_LIBDIR}/libtessera_capi.a" ${TESSERA_STATIC_LIBRARIES})
elseif(TESSERA_LINK STREQUAL "shared")
  set(TESSERA_LINK_LIBS ${TESSERA_LINK_LIBRARIES})
else()
  message(FATAL_ERROR "TESSERA_LINK must be 'shared' or 'static', not '${TESSERA_LINK}'")
endif()

foreach(prog smoke threads bench)
  add_executable(capi_${prog} ${prog}.cpp)
  target_include_directories(capi_${prog} PRIVATE ${TESSERA_INCLUDE_DIRS})
  target_compile_options(capi_${prog} PRIVATE -Wall -Wextra -Werror)
  target_link_libraries(capi_${prog} PRIVATE nanoarrow::nanoarrow ${TESSERA_LINK_LIBS} Threads::Threads)
endforeach()
