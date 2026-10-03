#include <windows.h>
#include <rpc.h>
#include <rpcdce.h>
#include <rpcnterr.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "browse_property_array_probe.h"

#define SERVER_RETRY_COUNT 100
#define SERVER_RETRY_DELAY_MS 50

void *__RPC_USER MIDL_user_allocate(size_t size)
{
    return malloc(size);
}

void __RPC_USER MIDL_user_free(void *memory)
{
    free(memory);
}

static int report_rpc_error(const char *operation, RPC_STATUS status)
{
    fprintf(stderr, "%s failed with RPC status %ld\n", operation, status);
    return 1;
}

static RPC_STATUS create_binding(const char *endpoint, RPC_BINDING_HANDLE *binding)
{
    RPC_CSTR string_binding = NULL;
    RPC_STATUS status = RpcStringBindingComposeA(
        NULL,
        (RPC_CSTR)"ncalrpc",
        NULL,
        (RPC_CSTR)endpoint,
        NULL,
        &string_binding);

    if (status != RPC_S_OK) {
        return status;
    }

    status = RpcBindingFromStringBindingA(string_binding, binding);
    (void)RpcStringFreeA(&string_binding);
    return status;
}

static RPC_STATUS call_browse(
    RPC_BINDING_HANDLE binding,
    unsigned long property_count,
    unsigned long *property_ids,
    long *result)
{
    for (unsigned int attempt = 0; attempt < SERVER_RETRY_COUNT; attempt++) {
        RPC_STATUS exception = RPC_S_OK;

        RpcTryExcept
        {
            *result = BrowsePropertyArrayProbe_Browse(
                binding,
                property_count,
                property_ids);
        }
        RpcExcept(1)
        {
            exception = RpcExceptionCode();
        }
        RpcEndExcept

        if (exception != RPC_S_SERVER_UNAVAILABLE && exception != RPC_S_UNKNOWN_IF) {
            return exception;
        }

        Sleep(SERVER_RETRY_DELAY_MS);
    }

    return RPC_S_SERVER_UNAVAILABLE;
}

static RPC_STATUS call_shutdown(RPC_BINDING_HANDLE binding)
{
    RPC_STATUS exception = RPC_S_OK;

    RpcTryExcept
    {
        BrowsePropertyArrayProbe_Shutdown(binding);
    }
    RpcExcept(1)
    {
        exception = RpcExceptionCode();
    }
    RpcEndExcept

    return exception;
}

int main(int argc, char **argv)
{
    RPC_BINDING_HANDLE binding = NULL;
    RPC_STATUS status;
    long result = 0;
    unsigned long empty_property_id = 0;

    if (argc != 3 || strcmp(argv[1], "client") != 0) {
        fputs("usage: browse_property_array_probe_client client <endpoint>\n", stderr);
        return 2;
    }

    status = create_binding(argv[2], &binding);
    if (status != RPC_S_OK) {
        return report_rpc_error("create_binding", status);
    }

    status = call_browse(binding, 0, NULL, &result);
    if (status != RPC_X_NULL_REF_POINTER) {
        (void)RpcBindingFree(&binding);
        fprintf(
            stderr,
            "zero-count null property pointer returned RPC status %ld and result %ld; "
            "expected RPC_X_NULL_REF_POINTER\n",
            status,
            result);
        return 1;
    }

    /* Count zero correlates an empty array; this valid address is not read or sent as an ID. */
    status = call_browse(binding, 0, &empty_property_id, &result);
    if (status != RPC_S_OK || result != 0) {
        (void)RpcBindingFree(&binding);
        fprintf(
            stderr,
            "zero-count non-null property pointer returned RPC status %ld and result %ld\n",
            status,
            result);
        return 1;
    }

    status = call_shutdown(binding);
    (void)RpcBindingFree(&binding);
    if (status != RPC_S_OK) {
        return report_rpc_error("call_shutdown", status);
    }

    puts("NDR rejected NULL for count zero and accepted a non-null zero-length property array");
    return 0;
}
