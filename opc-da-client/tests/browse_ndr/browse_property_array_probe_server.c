#include <windows.h>
#include <rpc.h>
#include <rpcdce.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "browse_property_array_probe.h"

void *__RPC_USER MIDL_user_allocate(size_t size)
{
    return malloc(size);
}

void __RPC_USER MIDL_user_free(void *memory)
{
    free(memory);
}

long BrowsePropertyArrayProbe_Browse(
    handle_t binding,
    unsigned long property_count,
    unsigned long *property_ids)
{
    (void)binding;

    /* A zero count means the NDR array has no elements, so do not dereference property_ids. */
    return property_count == 0 && property_ids != NULL ? 0 : 1;
}

void BrowsePropertyArrayProbe_Shutdown(handle_t binding)
{
    (void)binding;
    (void)RpcMgmtStopServerListening(NULL);
}

static int report_rpc_error(const char *operation, RPC_STATUS status)
{
    fprintf(stderr, "%s failed with RPC status %ld\n", operation, status);
    return 1;
}

int main(int argc, char **argv)
{
    RPC_STATUS status;

    if (argc != 3 || strcmp(argv[1], "server") != 0) {
        fputs("usage: browse_property_array_probe_server server <endpoint>\n", stderr);
        return 2;
    }

    status = RpcServerUseProtseqEpA(
        (RPC_CSTR)"ncalrpc",
        RPC_C_PROTSEQ_MAX_REQS_DEFAULT,
        (RPC_CSTR)argv[2],
        NULL);
    if (status != RPC_S_OK) {
        return report_rpc_error("RpcServerUseProtseqEpA", status);
    }

    status = RpcServerRegisterIf(
        BrowsePropertyArrayProbe_v1_0_s_ifspec,
        NULL,
        NULL);
    if (status != RPC_S_OK) {
        return report_rpc_error("RpcServerRegisterIf", status);
    }

    status = RpcServerListen(1, RPC_C_LISTEN_MAX_CALLS_DEFAULT, FALSE);
    if (status != RPC_S_OK) {
        return report_rpc_error("RpcServerListen", status);
    }

    status = RpcServerUnregisterIf(
        BrowsePropertyArrayProbe_v1_0_s_ifspec,
        NULL,
        TRUE);
    if (status != RPC_S_OK) {
        return report_rpc_error("RpcServerUnregisterIf", status);
    }

    return 0;
}
