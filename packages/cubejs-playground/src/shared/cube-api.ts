import cube, {
  CubeApi,
  ErrorResponse,
  HttpTransport,
  ITransportResponse,
  ITransportStreamParams,
  ITransportStreamResponse,
  TransportOptions,
} from '@cubejs-client/core';

import { isSessionToken, onUnauthorized, workspaceHeaders } from './auth';
import { isSameOrigin } from './helpers';

// HttpTransport that ends the session on a 401 from this server, when the
// request carried the session's token.
class PlaygroundTransport extends HttpTransport {
  private endsSession(): boolean {
    return isSameOrigin(this.apiUrl) && isSessionToken(this.authorization);
  }

  public request(apiMethod: string, params: any): ITransportResponse<Response> {
    const response = super.request(apiMethod, params);
    return {
      ...response,
      subscribe: <CBResult>(
        callback: (result: Response | ErrorResponse, resubscribe: () => Promise<CBResult>) => CBResult
      ) => response.subscribe<CBResult>((result, resubscribe) => {
        if (result && 'status' in result && this.endsSession()) {
          onUnauthorized(result);
        }
        return callback(result, resubscribe);
      }),
    };
  }

  // The SQL API stream (cubesql): HttpTransport reports a failed status only
  // as an `HTTP <status>: …` error.
  public requestStream<T extends Record<string, unknown> = Record<string, unknown>>(
    apiMethod: string,
    params: ITransportStreamParams<T>
  ): ITransportStreamResponse {
    const response = super.requestStream(apiMethod, params);
    return {
      ...response,
      stream: async () => {
        try {
          return await response.stream();
        } catch (e: any) {
          if (/^HTTP 401\b/.test(e?.message || '') && this.endsSession()) {
            onUnauthorized({ status: 401 });
          }
          throw e;
        }
      },
    };
  }
}

// The Cube API client for the Playground: data requests carry the chosen
// workspace (`X-Cube-Workspace`) when the API is this server.
export function createCubeApi(token: string, apiUrl: string): CubeApi {
  const headers: TransportOptions['headers'] = isSameOrigin(apiUrl) ? workspaceHeaders() : {};
  return cube(token, {
    apiUrl,
    transport: new PlaygroundTransport({ authorization: token, apiUrl, headers }),
  });
}
