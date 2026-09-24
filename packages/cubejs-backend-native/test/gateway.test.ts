import * as native from '../js';
import metaFixture from './meta';

const GATEWAY_PORT = 7580;

function gatewayMethods() {
  return {
    contextToApiScopes: jest.fn(async ({ securityContext }) => (
      securityContext?.scopes ?? ['data', 'meta']
    )),
    checkAuth: jest.fn(async ({ request, token }) => {
      expect(request).toEqual({ protocol: 'http' });

      if (token === 'meta-only') {
        return { securityContext: { scopes: ['meta'] } };
      }
      if (token === 'data-only') {
        return { securityContext: { scopes: ['data'] } };
      }
      if (token === 'valid') {
        return { securityContext: { foo: 'bar' } };
      }

      throw new Error('Invalid token');
    }),
    checkSqlAuth: jest.fn(async () => {
      throw new Error('checkSqlAuth must not be called by the API gateway');
    }),
    meta: jest.fn(async ({ onlyViews }) => ({
      ...metaFixture,
      cubes: onlyViews ? [] : metaFixture.cubes,
    })),
    sql: jest.fn(async () => ({ error: 'not implemented' })),
    sqlApiLoad: jest.fn(async () => ({ error: 'not implemented' })),
    stream: jest.fn(async () => ({ error: 'not implemented' })),
    sqlGenerators: jest.fn(async () => ({
      cubeNameToDataSource: {},
      memberToDataSource: {},
      dataSourceToSqlGenerator: {},
    })),
    logLoadEvent: jest.fn(),
    canSwitchUserForSession: jest.fn(() => true),
  };
}

const get = (path: string, token?: string) => fetch(`http://127.0.0.1:${GATEWAY_PORT}${path}`, {
  headers: token ? { authorization: `Bearer ${token}` } : {},
});

describe('Native API gateway', () => {
  jest.setTimeout(60 * 1000);

  let instance: native.SqlInterfaceInstance;
  let methods: ReturnType<typeof gatewayMethods>;

  beforeAll(async () => {
    methods = gatewayMethods();
    instance = await native.registerInterface({
      gatewayPort: GATEWAY_PORT,
      ...methods,
    });
  });

  afterAll(async () => {
    await native.shutdownInterface(instance, 'fast');
  });

  beforeEach(() => {
    Object.values(methods).forEach((fn) => fn.mockClear());
  });

  describe('GET /v1/meta', () => {
    it('returns cubes without compilerId', async () => {
      const res = await get('/v1/meta', 'valid');

      expect(res.status).toEqual(200);
      expect(res.headers.get('content-type')).toContain('application/json');
      expect(await res.json()).toEqual({ cubes: metaFixture.cubes });

      expect(methods.meta).toHaveBeenCalledTimes(1);
      expect(methods.meta.mock.calls[0][0]).toEqual({
        request: {
          id: expect.any(String),
          meta: null,
        },
        session: {
          user: null,
          superuser: false,
          securityContext: { foo: 'bar' },
        },
        onlyCompilerId: false,
      });
    });

    it('passes onlyViews=true through to the bridge', async () => {
      const res = await get('/v1/meta?onlyViews=true', 'valid');

      expect(res.status).toEqual(200);
      expect(await res.json()).toEqual({ cubes: [] });
      expect(methods.meta.mock.calls[0][0].onlyViews).toEqual(true);
    });

    it('rejects requests without authorization header', async () => {
      const res = await get('/v1/meta');

      expect(res.status).toEqual(401);
      expect(await res.json()).toEqual({ error: 'No authorization header' });
      expect(methods.meta).not.toHaveBeenCalled();
    });

    it('rejects invalid tokens', async () => {
      const res = await get('/v1/meta', 'invalid');

      expect(res.status).toEqual(401);
      expect(await res.json()).toEqual({ error: 'Authentication error' });
      expect(methods.meta).not.toHaveBeenCalled();
    });

    it('requires the meta API scope', async () => {
      const res = await get('/v1/meta', 'data-only');

      expect(res.status).toEqual(403);
      expect(await res.json()).toEqual({ error: 'API scope is missing: meta' });
      expect(methods.meta).not.toHaveBeenCalled();

      expect((await get('/v1/meta', 'meta-only')).status).toEqual(200);
    });

    it('does not implement ?extended yet', async () => {
      const res = await get('/v1/meta?extended', 'valid');

      expect(res.status).toEqual(501);
      expect(methods.meta).not.toHaveBeenCalled();
    });
  });
});
